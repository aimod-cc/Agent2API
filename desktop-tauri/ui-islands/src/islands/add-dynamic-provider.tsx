/** 未在内置表单登记的运行时 provider：动态方法、纯文本说明与旧插件兼容入口。 */
import * as React from 'react'
import {
  Alert, AlertDescription, AlertTitle, Button, Input, Label, SegmentedControl,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Spinner, Textarea,
} from '@ui'
import { AddFoot, AddGroup } from './add-account-kit'
import { t } from '../i18n'
import { MAX_NAME_LENGTH } from './add-account-configs'
import {
  accountRequest, addedLabelOf, afterAdd, describeError, isMethodsUnavailable,
  normalizeLoginResult, postAccount, runtimeProviderOf, shared, startLogin, toast,
  type LoginProgress, type LoginResult, type RuntimeProviderRecord,
  type WebLoginController, type WebLoginView,
} from './add-account-bridge'
import { draftOfProxy, ProxyForm, readProxyDraft } from './accounts-proxy-form'

import {
  loginText, methodLabel, parseCallbackInput, parseMethods, promptEnabled, promptValue, readInputs,
  type Method, type MethodsState,
} from './add-account-dynamic-fields'

const MAX_JSON_LENGTH = 262_144
const UNSAFE_KEYS = new Set(['__proto__', 'prototype', 'constructor'])
const RESERVED_KEYS = new Set([
  'provider', 'name', 'proxy', 'methodIndex', 'inputs', 'importDesktop', 'guest',
  'authFileContent', 'authFileB64', 'authFileName', 'edition', 'mode', 'socialRestore',
  'options', 'state', 'flow', 'callbackUrl', 'signal',
])
const textOf = (value: unknown): string => typeof value === 'string' ? value : ''
const objectOf = (value: unknown): Record<string, unknown> | undefined =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown> : undefined

/** 501 才使用已有握手 meta；不臆造方法索引，也不把 token/sms 当作 OAuth。 */
function legacyMethods(provider: RuntimeProviderRecord | undefined): MethodsState {
  const source = provider?.source || textOf(provider?.meta?.source)
  const builtin = source === 'builtin' || provider?.id?.endsWith('-subscription') === true
  if (builtin) return {
    phase: 'error', methods: [], instructions: '',
    reason: t('本体订阅未返回动态登录方式（501）；不会猜测导入接口或显示插件手动凭证入口。请更新网关后重试。'),
  }
  const meta: Record<string, unknown> | undefined = provider?.meta || (provider && { ...provider })
  const declared = meta?.login_methods ?? meta?.loginMethods
  const methods: Method[] = []
  for (const [index, value] of (Array.isArray(declared) ? declared : []).entries()) {
    const raw = objectOf(value)
    if (!raw) continue
    const kind = textOf(raw.kind)
    const method: Method = {
      id: `legacy:${index}`, type: 'unsupported', prompts: [], localOnly: false,
      manualCallback: false, callbackInput: 'url', embeddedOnly: false,
      label: textOf(raw.label) || textOf(raw.button) || kind,
      instructions: textOf(raw.hint) || textOf(raw.placeholder),
    }
    if (kind === 'browser') {
      method.type = 'oauth'
      method.label ||= t('网页登录（旧版声明）')
    } else if (kind === 'token') {
      // 旧 token 声明没有主字段的 key，不能猜成 token/apiKey；由 JSON 手动明确字段。
      method.type = 'manual'
      method.label ||= t('手动凭证 JSON')
      const extraFields = Array.isArray(raw.extra_fields) ? raw.extra_fields : []
      method.instructions = [method.instructions, ...extraFields.map(value => {
        const field = objectOf(value)
        return [textOf(field?.label), textOf(field?.key), textOf(field?.placeholder)].filter(Boolean).join(' · ')
      })].filter(Boolean).join('\n')
    } else if (kind === 'desktop') {
      method.type = 'desktop'
      method.localOnly = true
      method.label ||= t('从网关本机客户端导入')
    } else if (kind === 'guest') {
      method.type = 'guest'
      method.label ||= t('访客模式')
    } else {
      method.label ||= t('旧版登录方式')
      method.reason = kind === 'sms'
        ? t('插件只声明了短信登录，但没有下发可用的动态表单；此界面不会猜测短信接口')
        : t('此旧版登录声明无法安全映射到动态表单，请联系插件作者')
    }
    methods.push(method)
  }
  if (!methods.some(method => method.type === 'manual')) {
    methods.push({
      id: 'manual:compat', type: 'manual', label: t('手动凭证 JSON（兼容入口）'),
      prompts: [], localOnly: false, manualCallback: false, callbackInput: 'url', embeddedOnly: false, instructions: '',
    })
  }
  return {
    phase: 'legacy', methods, instructions: textOf(meta?.instructions),
    reason: Array.isArray(declared) && declared.length
      ? t('插件未实现 auth.methods（501）；以下入口仅依据旧版登录声明显示')
      : t('插件未实现 auth.methods（501），且没有可用的旧版登录声明；仅保留需要插件确认的手动凭证 JSON 入口'),
  }
}

function credentialPayload(text: string): Record<string, unknown> {
  if (!text.trim() || text.length > MAX_JSON_LENGTH) {
    throw new Error(t('请填写非空的凭证 JSON 对象，且不超过 256 KiB'))
  }
  let value: unknown
  try { value = JSON.parse(text) } catch {
    // JSON.parse 的错误可能包含原文片段，不能把凭证回显进错误提示。
    throw new Error(t('凭证 JSON 格式不正确，请检查括号和引号'))
  }
  const record = objectOf(value)
  if (!record || !Object.keys(record).length) throw new Error(t('凭证 JSON 必须是非空对象，不能是数组或字符串'))
  const inspect = (value: unknown, depth: number): void => {
    if (depth > 20) throw new Error(t('凭证 JSON 嵌套过深，请只粘贴该插件要求的凭证对象'))
    if (!value || typeof value !== 'object') return
    for (const [key, child] of Object.entries(value)) {
      if (UNSAFE_KEYS.has(key) || (depth === 0 && RESERVED_KEYS.has(key))) {
        throw new Error(t('凭证 JSON 含有不允许的字段：{key}', { key }))
      }
      inspect(child, depth + 1)
    }
  }
  inspect(record, 0)
  return record
}

function apiProxySelection(proxy: ReturnType<typeof readProxyDraft>): unknown {
  return proxy ?? { source: 'direct' }
}

export function DynamicProviderBlock({
  provider, label, active, version, footerHost,
}: {
  provider: string
  label: string
  active: boolean
  version: number
  footerHost: HTMLElement | null
}): React.ReactElement {
  const prefix = `${React.useId()}-dynamic-login`
  const [methodsState, setMethodsState] = React.useState<MethodsState>({
    phase: 'loading', methods: [], instructions: '', reason: '',
  })
  const [selected, setSelected] = React.useState('')
  const [values, setValues] = React.useState<Record<string, Record<string, string>>>({})
  const [credentialJson, setCredentialJson] = React.useState('')
  const [name, setName] = React.useState('')
  const [proxyDraft, setProxyDraft] = React.useState(() => draftOfProxy(undefined))
  const [mode, setMode] = React.useState<'external' | 'embedded'>('external')
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState('')
  const [progress, setProgress] = React.useState<LoginProgress>({})
  const [hasCallback, setHasCallback] = React.useState(false)
  const [callbackBusy, setCallbackBusy] = React.useState(false)
  const [callbackError, setCallbackError] = React.useState('')
  const [callbackNotice, setCallbackNotice] = React.useState('')
  const callbackInputRef = React.useRef<HTMLInputElement>(null)
  const callbackRequest = React.useRef(false)
  const callbackGeneration = React.useRef(0)
  const progressRef = React.useRef<LoginProgress>({})
  const [loginView, setLoginView] = React.useState<WebLoginView>({
    label: t('登录并添加'), hint: '', disabled: false, waiting: false,
  })
  const controllerRef = React.useRef<WebLoginController | null>(null)
  const alive = React.useRef(true)
  const savingRef = React.useRef(false)
  const requestingRef = React.useRef(false)
  const loadedRef = React.useRef(false)
  const method = methodsState.methods.find(item => item.id === selected) || methodsState.methods[0]
  const methodAvailable = method && methodsState.phase !== 'loading' && method.type !== 'unsupported'
  const selectedRef = React.useRef(method?.id)
  selectedRef.current = method?.id
  const inputs = (method && values[method.id]) || {}
  const locked = saving || loginView.disabled
  const legacyBrowser = method?.type === 'oauth' && method.methodIndex === undefined
  const snapshot = React.useRef({ method, inputs, mode, proxyDraft })
  snapshot.current = { method, inputs, mode, proxyDraft }
  const callbackType = progress.callbackInput ?? method?.callbackInput ?? 'url'
  // 网页端的粘贴框归 web shim；本岛仅补原生桌面等待态，避免重复回调输入。
  const manualWaiting = active && shared().workbuddyDesktop?.platform !== 'web'
    && loginView.waiting && method?.type === 'oauth' && !method.reason
    && progress.active === true && progress.provider === provider && Boolean(progress.state)
    && (progress.manualCallback === true || method.manualCallback)

  function clearCallback(preserveError = false): void {
    if (callbackInputRef.current) callbackInputRef.current.value = ''
    callbackGeneration.current += 1
    callbackRequest.current = false
    if (!alive.current) return
    setHasCallback(false)
    setCallbackBusy(false)
    if (!preserveError) setCallbackError('')
    setCallbackNotice('')
  }

  React.useEffect(() => {
    clearCallback(true)
    if (!active) {
      // 关掉弹窗即作废这次读取；下次打开会重新问插件要登录方式
      //（原来的「重新读取登录方式」按钮已移除，靠这里保证每次打开都是新清单）。
      loadedRef.current = false
      setCallbackError('')
      progressRef.current = {}
      setProgress({})
      setValues({})
      setCredentialJson('')
      setProxyDraft(previous => ({ ...previous, password: '' }))
    }
  }, [active, method?.id, progress.state, progress.callbackInput, manualWaiting])

  React.useEffect(() => {
    if (!active || savingRef.current || requestingRef.current || loginView.waiting) return
    if (loadedRef.current && runtimeProviderOf(provider)) return
    let current = true
    const previousSelection = selectedRef.current
    const previousInputs = snapshot.current.inputs
    const accept = (next: MethodsState): void => {
      if (!current) return
      const retained = next.methods.find(item => item.id === previousSelection)
      const nonSecret = retained ? Object.fromEntries(retained.prompts
        .filter(prompt => !prompt.secret && Object.hasOwn(previousInputs, prompt.key))
        .map(prompt => [prompt.key, previousInputs[prompt.key]])) : {}
      setValues(retained ? { [retained.id]: nonSecret } : {})
      setCredentialJson('')
      setSelected(retained?.id || '')
      setMethodsState(next)
      loadedRef.current = next.phase !== 'error'
    }
    requestingRef.current = true
    loadedRef.current = false
    setMethodsState(previous => ({ ...previous, phase: 'loading', reason: '' }))
    void (async () => {
      try {
        if (!runtimeProviderOf(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
        const result = await accountRequest('GET', `/api/session/login/methods?provider=${encodeURIComponent(provider)}`)
        accept(parseMethods(result, provider))
      } catch (failure) {
        if (!current) return
        accept(isMethodsUnavailable(failure) ? legacyMethods(runtimeProviderOf(provider)) : {
          phase: 'error', methods: [], instructions: '', reason: describeError(failure),
        })
      } finally {
        if (current) requestingRef.current = false
      }
    })()
    return () => { current = false; requestingRef.current = false }
    // 等待状态由共享引擎掌管，不以等待态重建方法或重新发起登录。
  }, [provider, active, version])

  React.useEffect(() => {
    alive.current = true
    const engine = shared().wbWebLogin
    if (engine) {
      controllerRef.current = engine.create({
        provider, buttonId: `${prefix}-start`, cancelId: `${prefix}-cancel`, hintId: `${prefix}-hint`,
        busyText: t('等待账号登录完成…'), onStateChange: setLoginView,
        texts: () => ({ button: t('登录并添加'), hint: '' }),
        start: async () => {
          const current = snapshot.current
          if (!current.method || current.method.type !== 'oauth') throw new Error(t('请先选择有效的 OAuth 登录方式'))
          setError('')
          clearCallback()
          progressRef.current = {}
          setProgress({})
          const inputs = readInputs(current.method, current.inputs)
          // 插件申报 embeddedOnly 时一律走内嵌窗口（不信任残留的 mode 状态）。
          const mode = current.method.embeddedOnly ? 'embedded' : current.mode
          // 旧 browser 声明没有索引，只走已存在的默认网页登录入口。
          return startLogin(provider, undefined, mode, undefined,
            current.method.methodIndex === undefined ? undefined : {
              methodIndex: current.method.methodIndex, inputs, proxy: readProxyDraft(current.proxyDraft),
            })
        },
        onSuccess: async raw => {
          const result = raw as LoginResult
          if (!result?.ok || result.canceled) throw new Error(t('登录桥未确认账号添加完成'))
          await finish(result)
        },
      }) ?? null
    }
    const receiveProgress = (next: LoginProgress): void => {
      if (!alive.current) return
      if (next.active === false || next.provider && next.provider !== provider) {
        progressRef.current = {}
        setProgress({})
        clearCallback()
        setValues({})
        setCredentialJson('')
        setProxyDraft(previous => ({ ...previous, password: '' }))
        return
      }
      if (next.provider !== provider || next.active !== true) return
      const current = snapshot.current.method
      const state = textOf(next.state)
      if (!state) {
        // start 的预约态 / 旧 LoginState 查询不携带 flow，不能覆盖已收到的真实进度。
        if (!progressRef.current.state) setProgress({ active: true, provider })
        return
      }
      let callbackInput: LoginProgress['callbackInput']
      try {
        callbackInput = parseCallbackInput(next.callbackInput === undefined ? current?.callbackInput : next.callbackInput)
        if (next.manualCallback !== undefined && typeof next.manualCallback !== 'boolean') throw new Error()
        if (current?.manualCallback && (next.manualCallback === false || callbackInput !== current.callbackInput)) throw new Error()
        if (callbackInput === 'code' && !(next.manualCallback === true || current?.manualCallback)) throw new Error()
      } catch {
        progressRef.current = {}
        setProgress({})
        clearCallback()
        setCallbackError(t('网关未返回有效的手动回调输入契约；请更新网关后重新发起。'))
        return
      }
      // 只摘公开进度字段，不保存整个 payload 或凭证；flow 必须属于当前 provider。
      const safe: LoginProgress = {
        active: true, provider, state,
        manualCallback: next.manualCallback === true, callbackInput,
        instructions: loginText(next.instructions), hint: loginText(next.hint), userCode: textOf(next.userCode),
      }
      if (progressRef.current.state !== safe.state || progressRef.current.callbackInput !== safe.callbackInput) clearCallback()
      progressRef.current = safe
      setProgress(safe)
    }
    const bridge = shared().workbuddyDesktop
    const unsubscribeState = bridge?.onLoginState?.(receiveProgress)
    const unsubscribeProgress = bridge?.onLoginProgress?.(receiveProgress)
    return () => {
      alive.current = false
      clearCallback()
      progressRef.current = {}
      controllerRef.current?.dispose()
      controllerRef.current = null
      if (typeof unsubscribeState === 'function') unsubscribeState()
      if (typeof unsubscribeProgress === 'function') unsubscribeProgress()
      void shared().wbWebLogin?.cancelIfActive?.(provider)
    }
  }, [provider, prefix])

  async function finish(result: LoginResult): Promise<void> {
    clearCallback()
    progressRef.current = {}
    if (!alive.current) { await shared().wbApp?.refresh?.(); return }
    setProgress({})
    setValues({})
    setCredentialJson('')
    setProxyDraft(draftOfProxy(undefined))
    setError('')
    const nickname = textOf(result.session?.nickname)
    await afterAdd(addedLabelOf(result.account) || nickname, label)
    if (result.warning) toast(t('账号已添加，但请留意：{warning}', { warning: result.warning }), 'warn')
  }

  async function submit(): Promise<void> {
    if (!method || locked || savingRef.current || requestingRef.current || methodsState.phase === 'loading') return
    try {
      if (!runtimeProviderOf(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
      if (method.reason) throw new Error(method.reason)
      const fields = readInputs(method, inputs)
      const proxy = readProxyDraft(proxyDraft)
      if (proxy && 'source' in proxy && proxy.source === 'rotate') {
        throw new Error(t('登录代理不支持轮询出网，请选择一条已保存的代理'))
      }
      const remark = method.prompts.some(prompt => prompt.key === 'name') ? fields.name : name.trim()
      const payload: Record<string, unknown> = { provider, proxy, ...(remark ? { name: remark } : {}) }
      if (method.type === 'api') {
        payload.methodIndex = method.methodIndex
        payload.inputs = fields
      } else if (method.type === 'manual') {
        Object.assign(payload, credentialPayload(credentialJson))
      } else if (method.type === 'desktop') {
        payload.importDesktop = true
      } else if (method.type === 'guest') {
        payload.guest = true
      } else {
        throw new Error(t('此登录方式需要由共享登录桥发起'))
      }
      savingRef.current = true
      setSaving(true)
      setError('')
      // API 方法必须进入 account.normalize，不能把 Key/Token 交给 OAuth start。
      const result = normalizeLoginResult(await postAccount(payload))
      if (method.type === 'api' && result.proxySaved !== true) {
        const id = result.account?.id
        let proxyWarning = t('旧网关未确认代理同次保存，已使用兼容接口保存；请在账号设置中确认代理选择。')
        try {
          if (!id) throw new Error(t('账号已添加，但未返回账号 ID，无法保存登录代理选择。'))
          await accountRequest('PATCH', `/api/accounts/${encodeURIComponent(id)}`, { proxy: apiProxySelection(proxy) })
        } catch {
          proxyWarning = t('账号已添加，但代理选择未确认保存；请在账号设置中重新检查。')
        }
        result.warning = [result.warning, proxyWarning].filter(Boolean).join('\n')
      }
      await finish(result)
    } catch (failure) {
      if (alive.current) setError(describeError(failure))
    } finally {
      savingRef.current = false
      if (alive.current) setSaving(false)
    }
  }

  function selectMethod(value: string): void {
    clearCallback()
    progressRef.current = {}
    setValues({})
    setCredentialJson('')
    setSelected(value)
    setError('')
    setProgress({})
  }

  function startOauth(): void {
    if (locked || !method || requestingRef.current || methodsState.phase === 'loading') return
    try {
      if (!runtimeProviderOf(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
      readInputs(method, inputs)
      if (!legacyBrowser) readProxyDraft(proxyDraft)
      if (!controllerRef.current) throw new Error(t('共享登录桥不可用，请重新打开面板'))
      controllerRef.current.start()
    } catch (failure) { setError(describeError(failure)) }
  }

  function cancelOauth(): void {
    clearCallback()
    setValues({})
    setCredentialJson('')
    setProxyDraft(previous => ({ ...previous, password: '' }))
    controllerRef.current?.cancel()
  }

  async function sendCallback(): Promise<void> {
    const current = snapshot.current.method
    const flow = progressRef.current
    const engine = shared().wbWebLogin
    if (callbackRequest.current || !manualWaiting || !current || flow.active !== true
      || flow.provider !== provider || !flow.state || current.reason
      || !(flow.manualCallback === true || current.manualCallback)) return
    const raw = callbackInputRef.current?.value || ''
    if (!raw) return
    const generation = callbackGeneration.current
    const isCurrent = () => alive.current && callbackGeneration.current === generation
      && progressRef.current.state === flow.state && progressRef.current.provider === provider
    callbackRequest.current = true
    setCallbackBusy(true)
    setCallbackError('')
    setCallbackNotice('')
    try {
      if (!runtimeProviderOf(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
      if (!engine?.isActive?.() || engine.activeProvider?.() !== provider) throw new Error()
      const bridge = shared().workbuddyDesktop
      if (!bridge?.submitLoginCallback) throw new Error()
      // URL / 裸 code 只在输入框与这次受保护 POST 中存在，不 trim、解析或伪造 URL。
      const reply = objectOf(await bridge.submitLoginCallback(flow.state, raw))
      if (!isCurrent()) return
      if (reply?.success === false || reply?.ok === false || reply?.error) throw new Error()
      clearCallback()
      setCallbackNotice(t('回调已提交，仍需等待网关确认账号登录完成。'))
      // accepted / Pending / accountId 都不触发 finish；终态只由现有 start/wait 链收尾。
    } catch {
      if (isCurrent()) setCallbackError(t('授权回调提交失败；请检查输入并重试，登录仍在等待。'))
    } finally {
      if (isCurrent()) {
        callbackRequest.current = false
        setCallbackBusy(false)
      }
    }
  }

  const instructionLines = [methodsState.instructions, method?.instructions,
    progress.instructions, progress.hint].filter((line): line is string => Boolean(line))
  const instructions = [...new Set(instructionLines)].join('\n')
  const commonFields = method && method.type !== 'unsupported' && !legacyBrowser

  return <div className='add-provider-block' hidden={!active}>
    <AddGroup label={t('账号登录方式')}>
      {methodsState.phase === 'loading' ? <p role='status'><Spinner /> {t('正在读取插件登录方式…')}</p>
        : methodsState.reason ? <Alert variant={methodsState.phase === 'error' ? 'destructive' : 'warning'}>
          <AlertTitle>{methodsState.phase === 'error' ? t('无法读取登录方式') : t('登录方式说明')}</AlertTitle>
          <AlertDescription className='whitespace-pre-wrap break-words'>{methodsState.reason}</AlertDescription>
        </Alert> : null}
      {method && methodsState.phase !== 'loading' ? (
        methodsState.methods.length <= 3 ? <SegmentedControl
          aria-label={t('{label} 账号的添加方式', { label })} disabled={locked}
          className='max-w-full flex-wrap [&_[data-slot=segmented-item]]:h-7'
          value={method.id} options={methodsState.methods.map(item => ({ value: item.id, label: methodLabel(item) }))}
          onValueChange={selectMethod}
        /> : <Select value={method.id} disabled={locked}
          onValueChange={value => { if (value !== null) selectMethod(value) }}>
          <SelectTrigger className='w-full' aria-label={t('账号登录方式')}>
            <SelectValue>{methodLabel(method)}</SelectValue>
          </SelectTrigger>
          <SelectContent>{methodsState.methods.map(item => <SelectItem key={item.id} value={item.id}>
            {methodLabel(item)}
          </SelectItem>)}</SelectContent>
        </Select>
      ) : null}
    </AddGroup>

    {instructions || progress.userCode ? <Alert>
      <AlertTitle>{t('登录指引 / 设备码授权')}</AlertTitle>
      <AlertDescription className='whitespace-pre-wrap break-words'>
        {instructions}
        {progress.userCode ? <div className='mt-2'>{t('设备授权码：{code}', { code: progress.userCode })}</div> : null}
      </AlertDescription>
    </Alert> : null}

    {method && methodsState.phase !== 'loading' ? <AddGroup label={methodLabel(method)}>
      {method.type === 'oauth' && !method.localOnly ? <p>{t('填写信息后发起授权，进度与失败原因会显示在这里。')}</p> : null}
      {method.type === 'api' ? <p>{t('API 方法提交给插件的账号规范化入口；不会用该密钥发起 OAuth 登录。')}</p> : null}
      {method.localOnly ? <Alert variant='warning'>
        <AlertTitle>{t('需要网关本机环境')}</AlertTitle>
        <AlertDescription>{t('“本机”指运行网关的机器，不是打开面板的电脑。远程浏览器也可以发起，但网关本机可能需要安装客户端并已登录；是否可用由网关返回结果决定。')}</AlertDescription>
      </Alert> : null}
      {method.manualCallback ? <Alert>
        <AlertDescription>{method.callbackInput === 'code'
          ? t('此方式需要原样提交授权码或令牌；输入会隐藏，不会当作 URL 或 API 凭证解析。提交后仍需等待网关确认。')
          : t('此方式需要完整回调地址；请原样粘贴，不拆分参数或重建地址。提交后仍需等待网关确认。')}</AlertDescription>
      </Alert> : null}
      {method.reason ? <Alert variant='warning'><AlertDescription>{method.reason}</AlertDescription></Alert> : null}
      {method.prompts.map((prompt, index) => {
        if (!promptEnabled(prompt, method, inputs)) return null
        const id = `${prefix}-prompt-${index}`
        const current = promptValue(prompt, inputs)
        return <div key={`${method.id}:${prompt.key}`} className='flex flex-col gap-1.5'>
          <Label htmlFor={id}>{prompt.label}{prompt.optional ? t('（可选）') : t('（必填）')}</Label>
          {prompt.type === 'select' ? <Select value={current} disabled={locked || !prompt.options.length}
            onValueChange={value => setValues(previous => ({
              ...previous, [method.id]: { ...previous[method.id], [prompt.key]: value ?? '' },
            }))}>
            <SelectTrigger id={id} className='w-full'>
              <SelectValue>{prompt.options.find(option => option.value === current)?.label
                || prompt.placeholder || t('请选择')}</SelectValue>
            </SelectTrigger>
            <SelectContent>
              {prompt.optional ? <SelectItem value=''>{t('不填写（可选）')}</SelectItem> : null}
              {prompt.options.map(option => <SelectItem key={option.value} value={option.value}>{option.label}</SelectItem>)}
            </SelectContent>
          </Select> : <Input id={id} type={prompt.secret ? 'password' : 'text'} disabled={locked}
            autoComplete={prompt.secret ? 'new-password' : 'off'} spellCheck={false}
            placeholder={prompt.placeholder} value={current} onChange={event => {
              const value = event.currentTarget.value
              setValues(previous => ({ ...previous, [method.id]: { ...previous[method.id], [prompt.key]: value } }))
            }} />}
        </div>
      })}
      {method.type === 'manual' ? <>
        <Alert variant='warning'><AlertDescription>
          {t('这是旧插件的手动兼容入口，不代表支持任意凭证或登录方式。请仅粘贴该插件明确接受的凭证 JSON；网关会交给 account.normalize 校验，不会自动推断 Token、Cookie 或 OAuth。')}
        </AlertDescription></Alert>
        <Label htmlFor={`${prefix}-credential`}>{t('凭证 JSON（必填）')}</Label>
        <Textarea id={`${prefix}-credential`} rows={6} maxLength={MAX_JSON_LENGTH} disabled={locked}
          autoComplete='off' spellCheck={false} value={credentialJson}
          onChange={event => setCredentialJson(event.currentTarget.value)}
          placeholder={t('粘贴插件要求的 JSON 对象，不要包含 provider、name、proxy 或登录流程控制字段')} />
        <p>{t('凭证只在提交时发送到网关；备注名和代理请使用下面的字段，解析错误不会回显凭证内容。')}</p>
      </> : null}
      {commonFields ? <>
        {method.type !== 'oauth' && !method.prompts.some(prompt => prompt.key === 'name') ? <div className='flex flex-col gap-1.5'>
          <Label htmlFor={`${prefix}-name`}>{t('备注名')}</Label>
          <Input id={`${prefix}-name`} maxLength={MAX_NAME_LENGTH} placeholder={t('可选，留空使用账号昵称')}
            value={name} disabled={locked} onChange={event => setName(event.currentTarget.value)} />
        </div> : null}
        <div inert={locked} className='flex min-w-0 flex-col gap-2'>
          <Label>{t('账号 / 登录出网代理')}</Label>
          <ProxyForm draft={proxyDraft} onChange={setProxyDraft} idPrefix={`${prefix}-proxy`} />
        </div>
        {method.type === 'api' ? <p>{t('新网关确认代理同次保存后不再重复提交；旧网关使用兼容保存并提示核查，失败不会撤销已添加的账号。')}</p> : null}
      </> : null}
      {legacyBrowser ? <p>{t('旧版浏览器登录暂不接收备注名和登录代理，请添加后在账号设置中调整。')}</p> : null}
      {method.type === 'oauth' && !legacyBrowser && !method.prompts.some(prompt => prompt.key === 'name')
        ? <p>{t('账号名由插件自动获取，添加后可修改。')}</p> : null}
      {method.type === 'oauth' && !method.localOnly && !method.embeddedOnly && shared().workbuddyDesktop?.platform !== 'web' ? <>
        <Label>{t('打开方式')}</Label>
        <SegmentedControl aria-label={t('网页登录的打开方式')} value={mode} disabled={locked}
          options={[{ value: 'external', label: t('系统浏览器') }, { value: 'embedded', label: t('内嵌窗口') }]}
          onValueChange={setMode} />
      </> : null}
      {method.type === 'oauth' ? <AddFoot host={footerHost} active={active}
        note={<span id={`${prefix}-hint`} className='detail whitespace-pre-wrap break-words'>{loginView.hint}</span>}>
        <Button id={`${prefix}-start`} disabled={locked || Boolean(method.reason)} onClick={startOauth}>
          {loginView.waiting ? <Spinner /> : null}{loginView.label}
        </Button>
        <Button id={`${prefix}-cancel`} variant='outline' hidden={!loginView.waiting}
          onClick={cancelOauth}>{t('取消等待')}</Button>
      </AddFoot> : method.type !== 'unsupported' ? <AddFoot host={footerHost} active={active}>
        <Button disabled={locked || Boolean(method.reason)}
          onClick={() => void submit()}>{saving ? <Spinner /> : null}{saving ? t('添加中…') : t('添加账号')}</Button>
      </AddFoot> : null}
      {manualWaiting ? <div className='flex flex-col gap-2'>
        <Label htmlFor={`${prefix}-callback`}>{callbackType === 'code'
          ? t('授权码或令牌（原样提交）') : t('完整回调地址（原样提交）')}</Label>
        <Input key={`${progress.state}:${callbackType}`} id={`${prefix}-callback`} ref={callbackInputRef}
          type={callbackType === 'code' ? 'password' : 'text'} defaultValue=''
          autoComplete={callbackType === 'code' ? 'new-password' : 'off'} spellCheck={false} autoCapitalize='none'
          disabled={callbackBusy} placeholder={callbackType === 'code'
            ? t('授权码或令牌（原样提交）') : t('完整回调地址（原样提交）')}
          onChange={event => {
            setHasCallback(event.currentTarget.value.length > 0)
            setCallbackError('')
            setCallbackNotice('')
          }} />
        <Button variant='outline' className='self-start' disabled={!hasCallback || callbackBusy}
          onClick={() => void sendCallback()}>{callbackBusy ? <Spinner /> : null}{t('提交授权回调')}</Button>
        {callbackNotice ? <p role='status'>{callbackNotice}</p> : null}
      </div> : null}
      {callbackError ? <Alert variant='destructive'><AlertDescription>{callbackError}</AlertDescription></Alert> : null}
      {error ? <Alert variant='destructive'><AlertTitle>{t('添加失败')}</AlertTitle>
        <AlertDescription className='whitespace-pre-wrap break-words'>{error}</AlertDescription>
      </Alert> : null}
    </AddGroup> : null}
    {error && !methodAvailable ? <Alert variant='destructive'><AlertDescription>{error}</AlertDescription></Alert> : null}
  </div>
}
