/** 运行时提供商的动态登录表单。 */
import * as React from 'react'
import {
  Alert, AlertDescription, AlertTitle, Button, Input, Label, Spinner,
} from '@ui'
import { AddFoot } from './add-account-kit'
import { t } from '../i18n'
import { MAX_NAME_LENGTH } from './add-account-constants'
import {
  accountRequest, addedLabelOf, afterAdd, describeError, desktopImportRequest, isMethodsUnavailable,
  normalizeLoginResult, postAccount, accountProviderAvailable, saveAccountProxySelection, shared, startLogin, toast,
  type LoginProgress, type LoginResult,
  type WebLoginController, type WebLoginView,
} from './add-account-bridge'
import { readProxyDraft } from './accounts-proxy-form'
import { useAddAccountProxy, useAddAccountProxyDraft } from './add-account-proxy'
import { useAddAccountSettings } from './add-account-settings'
import { SettingGroup } from './accounts-settings-fields'
import { providerFeatures } from './accounts-features'
import { userAgentEditable, createdAccountIdOf } from './add-account-user-agent'
import { openExternal } from './update-shared'
import { PluginLoginInteraction, type LoginRequest } from '../../../ui-shared/plugin-login-interaction'
import { PluginDesktopImport } from '../../../ui-shared/plugin-desktop-import'
import { getPluginStartup, subscribePluginStartup } from '../../../ui-shared/plugin-startup'
import { managedLoginSourceGeneration, MANAGEMENT_SESSION_CHANGE, type ManagedLoginScope } from '../../../ui-shared/plugin-login-managed'
import { PluginLoginBrowser, PluginLoginFields, PluginLoginMethodPicker, PluginLoginSecurity } from '../../../ui-shared/plugin-login-form'
import { PluginLoginExternalLink } from '../../../ui-shared/plugin-login-external-link'
import { useLoginNoAutoOpen } from '../../../ui-shared/login-browser-prefs'
import {
  credentialFilePayloadForItem, credentialInputMethod, inspectSoftwareExport,
  type CredentialUploadItem,
} from '../../../ui-shared/plugin-credential-file'
import {
  buildCredentialAccountPayload, finalizeBatchImport, isProviderFirstAccount,
  resolveBatchImportItems, runCredentialBatchImport,
  type BatchImportFailure, type BatchImportProgress,
} from './add-dynamic-batch'
import { PluginLoginDiagnostic } from '../../../ui-shared/plugin-login-diagnostic-view'
import { loginDiagnosticOf, type LoginDiagnostic } from '../../../ui-shared/plugin-login-diagnostic'
import { usePluginPrompts } from './add-account-plugin-prompts'

import {
  loginText, mergeLoginInstructions, parseCallbackInput, parseMethods, pluginBrowserInstructions, readInputs,
  type MethodsState,
} from './add-account-dynamic-fields'

const textOf = (value: unknown): string => typeof value === 'string' ? value : ''
const objectOf = (value: unknown): Record<string, unknown> | undefined =>
  value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown> : undefined
const safeLoginUrl = (value: unknown): string => {
  if (typeof value !== 'string') return ''
  const raw = value.trim()
  if (!raw || raw.length > 16384 || /[\u0000-\u0020\u007f]/.test(raw)) return ''
  try {
    const parsed = new URL(raw)
    if (!['https:', 'http:'].includes(parsed.protocol) || !parsed.hostname || parsed.username || parsed.password) return ''
    return raw
  } catch { return '' }
}

export function DynamicProviderBlock({
  provider, label, active, version, footerHost, preserveOnHide = false, availability = '',
}: {
  provider: string
  label: string
  active: boolean
  version: number
  footerHost: HTMLElement | null
  preserveOnHide?: boolean
  availability?: string
}): React.ReactElement {
  const prefix = `${React.useId()}-dynamic-login`
  const [methodsState, setMethodsState] = React.useState<MethodsState>({
    phase: 'loading', methods: [], instructions: '', reason: '',
  })
  const [selected, setSelected] = React.useState('')
  const [values, setValues] = React.useState<Record<string, Record<string, string>>>({})
  const [uploads, setUploads] = React.useState<CredentialUploadItem[]>([])
  const [importProgress, setImportProgress] = React.useState<BatchImportProgress | null>(null)
  const [batchFailures, setBatchFailures] = React.useState<BatchImportFailure[]>([])
  const [name, setName] = React.useState('')
  const proxy = useAddAccountProxy()
  const { draft: proxyDraft, change: setProxyDraft } = useAddAccountProxyDraft()
  const settings = useAddAccountSettings()
  const settingsRef = React.useRef(settings)
  settingsRef.current = settings
  const supportsUsage = providerFeatures(provider).usage
  const uaEditable = userAgentEditable(provider)
  const isDesktop = shared().workbuddyDesktop?.platform !== 'web'
  const defaultBrowserMode: 'external' | 'embedded' = isDesktop ? 'embedded' : 'external'
  const [mode, setMode] = React.useState<'external' | 'embedded'>(defaultBrowserMode)
  const [noAutoOpen, setNoAutoOpen] = useLoginNoAutoOpen()
  const previousActiveRef = React.useRef(active)
  const [saving, setSaving] = React.useState(false)
  const [error, updateError] = React.useState('')
  const [diagnostic, setDiagnostic] = React.useState<LoginDiagnostic>()
  const setError = (message: string, source?: unknown): void => {
    updateError(message)
    setDiagnostic(message ? loginDiagnosticOf(source) : undefined)
  }
  const [progress, setProgress] = React.useState<LoginProgress>({})
  const [hasCallback, setHasCallback] = React.useState(false)
  const [callbackBusy, setCallbackBusy] = React.useState(false)
  const [callbackError, setCallbackError] = React.useState('')
  const [callbackNotice, setCallbackNotice] = React.useState('')
  const [nextAuthUrl, setNextAuthUrl] = React.useState('')
  const callbackInputRef = React.useRef<HTMLInputElement>(null)
  const callbackRequest = React.useRef(false)
  const callbackGeneration = React.useRef(0)
  const progressRef = React.useRef<LoginProgress>({})
  const loginScope = React.useRef<{
    origin: string; bridge: ReturnType<typeof shared>['workbuddyDesktop']; bootId: string; managementGeneration: number;
    state: string; requestId: string; methodIndex?: number; remark?: string; managed: boolean; cancelled: boolean; cancelSent?: boolean; deadline: number; cleanup?: () => void;
  } | null>(null)
  const [loginView, setLoginView] = React.useState<WebLoginView>({
    label: t('登录并添加'), hint: '', disabled: false, waiting: false,
  })
  const controllerRef = React.useRef<WebLoginController | null>(null)
  const alive = React.useRef(true)
  const savingRef = React.useRef(false)
  const requestingRef = React.useRef(false)
  const loadedRef = React.useRef(false)
  const loadedAvailability = React.useRef('')
  const localActionsAvailable = shared().workbuddyDesktop?.platform === 'windows'
    && Boolean(shared().__TAURI_INTERNALS__?.invoke) && accountProviderAvailable(provider)
  const localAction = localActionsAvailable && methodsState.phase === 'ready' && provider === 'chatglm' ? 'local:chatglm-import' : ''
  const localActionSelected = Boolean(localAction && selected === localAction)
  const declaredMethod = localActionSelected ? undefined
    : methodsState.methods.find(item => item.id === selected) || methodsState.methods.find(item => !item.reason) || methodsState.methods[0]
  const inputs = (declaredMethod && values[declaredMethod.id]) || {}
  const resolvedPrompts = usePluginPrompts(provider, declaredMethod, inputs, active && !loginView.waiting,
    (path, body) => accountRequest('POST', path, body))
  const method = resolvedPrompts.method
  const methodAvailable = method && methodsState.phase === 'ready'
  const selectedRef = React.useRef(selected)
  selectedRef.current = selected || method?.id || ''
  const locked = saving || loginView.disabled || proxy.busy
  const [readingFile, setReadingFile] = React.useState(false)
  const submitLocked = locked || readingFile || resolvedPrompts.pending || Boolean(resolvedPrompts.error)
  const remarkRef = React.useRef(name)
  remarkRef.current = name
  const usesExternalBrowser = Boolean(
    method && method.type === 'oauth' && !method.embeddedOnly
    && method.browserMode !== 'none' && method.browserMode !== 'plugin'
    && !method.localOnly && (!isDesktop || method.browserMode === 'external' || mode === 'external'),
  )
  const snapshot = React.useRef({ method, inputs, mode, proxyDraft, noAutoOpen, usesExternalBrowser })
  snapshot.current = { method, inputs, mode, proxyDraft, noAutoOpen, usesExternalBrowser }
  const callbackType = progress.callbackInput ?? method?.callbackInput ?? 'url'
  // 网页端的粘贴框归 web shim；本岛仅补原生桌面等待态，避免重复回调输入。
  const manualWaiting = (active || preserveOnHide) && shared().workbuddyDesktop?.platform !== 'web'
    && loginView.waiting && method?.type === 'oauth' && !method.reason
    && progress.active === true && progress.provider === provider && Boolean(progress.state)
    && progress.managedLogin !== true && (progress.manualCallback === true || method.manualCallback)

  const bootId = () => getPluginStartup().value?.bootId || shared().wbApp?.getState?.()?.pluginStartup?.bootId || ''
  const sameServer = (scope: NonNullable<typeof loginScope.current>) => scope.bridge === shared().workbuddyDesktop
    && scope.origin === window.location.origin && scope.bootId === bootId() && scope.managementGeneration === managedLoginSourceGeneration()
    && (!shared().wbApp?.getState?.()?.pluginStartup?.bootId || scope.bootId === shared().wbApp?.getState?.()?.pluginStartup?.bootId)
  const savingUnconfirmed = (value: LoginProgress) => value.managedLogin === true
    && (value.phase === 'saving' || value.phase === 'unconfirmed' || value.saved === null)
  const resultCurrent = (scope: NonNullable<typeof loginScope.current>) => alive.current && loginScope.current === scope
    && !scope.cancelled && sameServer(scope) && accountProviderAvailable(provider)
  const scopeCurrent = (scope: NonNullable<typeof loginScope.current>) => resultCurrent(scope)
    && (!scope.managed || performance.now() < scope.deadline)

  const nextLoginRequestId = () => typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
    ? `req-${crypto.randomUUID()}`
    : `req-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`

  async function cancelOwnedLogin(automatic = false): Promise<unknown> {
    const scope = loginScope.current
    if (!scope || scope.cancelled) return false
    const savingUnknown = savingUnconfirmed(progressRef.current) || progressRef.current.saved === true
    scope.cancelled = true
    if (alive.current) { progressRef.current = {}; setProgress({}) }
    if (automatic && savingUnknown) { scope.cancelSent = true; scope.cleanup?.(); return false }
    if (!sameServer(scope)) return false
    const target = scope.state || scope.requestId
    if (target) {
      scope.cleanup?.()
      scope.cancelSent = true
      const result = scope.bridge?.cancelLogin
        ? await scope.bridge.cancelLogin(target)
        : await accountRequest('POST', '/api/session/login/cancel', { state: target })
      if (result && typeof result === 'object' && 'canceled' in result) {
        return (result as { canceled?: boolean }).canceled !== false
      }
      return result !== false
    }
    return scope.bridge?.cancelLogin?.()
  }

  const interactionRequest: LoginRequest = async <T,>(verb: 'GET' | 'POST' | 'PATCH', path: string, body?: unknown, options?: { signal?: AbortSignal }) => {
    const scope = loginScope.current
    if (!scope || !scopeCurrent(scope) || savingUnconfirmed(progressRef.current) || options?.signal?.aborted) throw new DOMException('请求已取消', 'AbortError')
    const response = await accountRequest(verb, path, body)
    if (!scopeCurrent(scope) || savingUnconfirmed(progressRef.current) || options?.signal?.aborted) throw new DOMException('请求已取消', 'AbortError')
    return response as T
  }

  function clearCallback(preserveError = false): void {
    if (callbackInputRef.current) callbackInputRef.current.value = ''
    callbackGeneration.current += 1
    callbackRequest.current = false
    if (!alive.current) return
    setHasCallback(false)
    setCallbackBusy(false)
    if (!preserveError) setCallbackError('')
    setCallbackNotice('')
    setNextAuthUrl('')
  }

  const callbackIdentity = `${method?.id || ''}:${progress.state || ''}:${callbackType}:${manualWaiting}`
  const previousCallback = React.useRef(callbackIdentity)
  React.useEffect(() => {
    if (preserveOnHide && previousCallback.current === callbackIdentity) return
    previousCallback.current = callbackIdentity
    clearCallback(true)
    if (!active && !preserveOnHide) {
      // 关掉弹窗即作废这次读取；下次打开会重新问插件要登录方式
      //（原来的「重新读取登录方式」按钮已移除，靠这里保证每次打开都是新清单）。
      loadedRef.current = false
      setCallbackError('')
      progressRef.current = {}
      setProgress({})
      setValues({})
      setName('')
      setUploads([])
      setImportProgress(null)
      setBatchFailures([])
      setProxyDraft({ ...snapshot.current.proxyDraft, password: '' })
    }
  }, [active, preserveOnHide, callbackIdentity])

  React.useEffect(() => {
    if (!active || savingRef.current || requestingRef.current || loginView.waiting) return
    if (loadedRef.current && loadedAvailability.current === availability && accountProviderAvailable(provider)) return
    let current = true
    const previousSelection = selectedRef.current
    const previousInputs = snapshot.current.inputs
    const accept = (next: MethodsState): void => {
      if (!current) return
      next.methods = next.methods.filter(method => provider !== 'chatglm' || method.type !== 'desktop')
      const retained = next.methods.find(item => item.id === previousSelection)
      const nonSecret = retained ? Object.fromEntries(retained.prompts
        .filter(prompt => !prompt.secret && Object.hasOwn(previousInputs, prompt.key))
        .map(prompt => [prompt.key, previousInputs[prompt.key]])) : {}
      setValues(retained ? { [retained.id]: nonSecret } : {})
      setUploads([])
      setImportProgress(null)
      setBatchFailures([])
      setSelected(retained?.id || (previousSelection?.startsWith('local:') ? previousSelection : ''))
      setMethodsState(next)
      loadedRef.current = next.phase !== 'error'
      loadedAvailability.current = availability
    }
    requestingRef.current = true
    loadedRef.current = false
    setMethodsState(previous => ({ ...previous, phase: 'loading', reason: '' }))
    void (async () => {
      try {
        if (!accountProviderAvailable(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
        const locale = window.wbI18n?.locale() || 'zh-CN'
        const result = await accountRequest('GET', `/api/session/login/methods?provider=${encodeURIComponent(provider)}&locale=${encodeURIComponent(locale)}`)
        const parsed = parseMethods(result, provider)
        if (shared().workbuddyDesktop?.platform === 'web') parsed.methods = parsed.methods.filter(method => !method.desktopOnly)
        accept(parsed)
      } catch (failure) {
        if (!current) return
        accept({
          phase: 'error', methods: [], instructions: '', reason: isMethodsUnavailable(failure)
            ? t('该提供商未声明动态登录方式，请更新插件或网关后重试；不会使用旧版兼容入口。')
            : describeError(failure),
        })
      } finally {
        if (current) requestingRef.current = false
      }
    })()
    return () => { current = false; requestingRef.current = false }
    // 等待状态由共享引擎掌管，不以等待态重建方法或重新发起登录。
  }, [provider, active, version, availability])

  React.useEffect(() => {
    const becameActive = !previousActiveRef.current && active
    previousActiveRef.current = active
    if (becameActive && !loginView.waiting) setMode(defaultBrowserMode)
  }, [active, defaultBrowserMode, loginView.waiting])

  React.useEffect(() => {
    controllerRef.current?.syncTexts()
  }, [usesExternalBrowser, noAutoOpen])

  React.useEffect(() => {
    alive.current = true
    const engine = shared().wbWebLogin
    if (engine) {
      controllerRef.current = engine.create({
        provider, buttonId: `${prefix}-start`, cancelId: `${prefix}-cancel`, hintId: `${prefix}-hint`,
        busyText: t('等待账号登录完成…'), onStateChange: setLoginView,
        texts: () => ({
          button: snapshot.current.usesExternalBrowser && snapshot.current.noAutoOpen ? t('获取授权链接') : t('登录并添加'),
          hint: '',
        }),
        start: async () => {
          const current = snapshot.current
          if (!current.method || !['oauth', 'sms'].includes(current.method.type)) throw new Error(t('请先选择有效的登录方式'))
          setError('')
          clearCallback()
          progressRef.current = {}
          setProgress({})
          const inputs = readInputs(current.method, current.inputs)
          const requestId = nextLoginRequestId()
          loginScope.current = { origin: window.location.origin, bridge: shared().workbuddyDesktop, bootId: bootId(), managementGeneration: managedLoginSourceGeneration(),
            state: '', requestId, methodIndex: current.method.methodIndex,
            remark: current.method.type === 'sms' || current.method.prompts.some(prompt => prompt.key === 'name') ? '' : remarkRef.current.trim(),
            managed: false, cancelled: false, deadline: Infinity }
          const owner = loginScope.current
          // 插件申报 embeddedOnly 时一律走内嵌窗口（不信任残留的 mode 状态）。
          const mode = current.method.embeddedOnly ? 'embedded' : current.method.browserMode === 'external' ? 'external' : current.mode
          try {
            const result = await startLogin(provider, undefined, mode, undefined, {
              methodIndex: current.method.methodIndex, inputs, locale: window.wbI18n?.locale() || 'zh-CN',
              proxy: readProxyDraft(current.proxyDraft), requestId,
              autoOpenBrowser: mode === 'external' ? !current.noAutoOpen : true,
            })
            if (owner.managed && result.managedLogin !== true) throw new Error(t('账号保存未确认，请查询本次状态；不会再次保存。'))
            if (result.managedLogin === true && (result.provider !== provider || result.methodIndex !== owner.methodIndex
              || owner.state && result.flow !== owner.state)) throw new Error(t('账号保存未确认，请查询本次状态；不会再次保存。'))
            return resultCurrent(owner) ? result : { ok: false, canceled: true, silent: true }
          } catch (failure) {
            if (!resultCurrent(owner)) return { ok: false, canceled: true, silent: true }
            if (alive.current) setError(loginText(describeError(failure)), failure)
            throw failure
          } finally { owner.cleanup?.() }
        },
        cancel: () => cancelOwnedLogin(),
        onSuccess: async raw => {
          const owner = loginScope.current
          if (!owner || !resultCurrent(owner)) return
          const result = raw as LoginResult
          if (!result?.ok || result.canceled) throw new Error(t('登录桥未确认账号添加完成'))
          if (owner.managed && result.managedLogin !== true) throw new Error(t('账号保存未确认，请查询本次状态；不会再次保存。'))
          if (result.managedLogin === true && (result.provider !== provider || result.methodIndex !== owner.methodIndex
            || owner.state && result.flow !== owner.state || result.saved !== true || result.phase !== 'saved'
            || !result.session || !(result.session.accountUid || result.session.accountId))) {
            throw new Error(t('账号保存未确认，请查询本次状态；不会再次保存。'))
          }
          if (owner.remark) {
            const id = createdAccountIdOf(result), remark = owner.remark
            owner.remark = ''
            if (id && resultCurrent(owner)) {
              try { await accountRequest('PATCH', `/api/accounts/${encodeURIComponent(id)}`, { name: remark }) }
              catch { if (resultCurrent(owner)) toast(t('账号已添加，但备注名未保存，请在账号设置中修改。'), 'warn') }
            }
          }
          const settingsWarning = await settingsRef.current.applySettingsAfterCreate(
            createdAccountIdOf(result),
            provider,
            { supportsUsage, uaEditable },
          )
          if (!resultCurrent(owner)) return
          await finish(result, settingsWarning)
        },
      }) ?? null
    }
    const receiveProgress = (next: LoginProgress): void => {
      if (!alive.current) return
      const scope = loginScope.current
      if (next.active === false && scope?.managed && savingUnconfirmed(progressRef.current)
        && (!next.state || next.state === scope.state) && (next.provider === undefined || next.provider === provider)
        && next.phase === undefined && next.saved === undefined) return
      if ((next.active === false && !savingUnconfirmed(next)) || (next.provider && next.provider !== provider)) {
        if (preserveOnHide && progressRef.current.active !== true) return
        if (next.state && progressRef.current.state && next.state !== progressRef.current.state) return
        progressRef.current = {}
        setProgress({})
        clearCallback()
        setValues({})
        setUploads([])
        setImportProgress(null)
        setBatchFailures([])
        setProxyDraft({ ...snapshot.current.proxyDraft, password: '' })
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
      if (!scope || !sameServer(scope)) return
      if (scope.managed && next.managedLogin !== true) return
      if (scope.state && scope.state !== state) return
      if (!scope.state) scope.state = state
      if (scope.cancelled) {
        if (!scope.cancelSent) {
          scope.cancelSent = true
          void accountRequest('POST', '/api/session/login/cancel', { state }).catch(() => {})
        }
        return
      }
      if (next.managedLogin === true) {
        const savingUnknown = savingUnconfirmed(next)
        if (next.methodIndex !== scope.methodIndex || typeof next.remainingMs !== 'number' || !Number.isFinite(next.remainingMs)
          || (savingUnknown ? next.remainingMs < 0 : next.remainingMs <= 0) || !scope.bootId || next.manualCallback !== false || next.callbackInput !== 'url') {
          if (!savingUnknown) void cancelOwnedLogin().catch(() => {})
          clearCallback(); setValues({})
          setError(savingUnknown ? t('账号保存未确认，请查询本次状态；不会再次保存。') : t('登录步骤与当前流程不匹配，请重新发起登录。'))
          return
        }
        scope.managed = true
        scope.deadline = Math.min(scope.deadline, performance.now() + next.remainingMs)
      }
      let callbackInput: LoginProgress['callbackInput']
      try {
        callbackInput = parseCallbackInput(next.callbackInput === undefined ? current?.callbackInput : next.callbackInput)
        if (next.manualCallback !== undefined && typeof next.manualCallback !== 'boolean') throw new Error()
        if (!scope.managed && current?.manualCallback && (next.manualCallback === false || callbackInput !== current.callbackInput)) throw new Error()
        if (callbackInput === 'code' && !(next.manualCallback === true || current?.manualCallback)) throw new Error()
      } catch {
        progressRef.current = {}
        setProgress({})
        clearCallback()
        setCallbackError(t('网关未返回有效的手动回调输入契约；请更新网关后重新发起。'))
        return
      }
      // 只摘公开进度字段及已校验的外部授权链接，不保存整个 payload 或凭证；flow 必须属于当前 provider。
      const sameFlow = progressRef.current.state === state && progressRef.current.provider === provider
      const incomingUrl = !scope.managed ? safeLoginUrl(next.authUrl) : ''
      const authUrl = incomingUrl || (sameFlow && !scope.managed ? progressRef.current.authUrl : undefined)
      const browserOpened = incomingUrl
        ? (typeof next.browserOpened === 'boolean' ? next.browserOpened : undefined)
        : (sameFlow && !scope.managed ? progressRef.current.browserOpened : undefined)
      const safe: LoginProgress = {
        active: true, provider, state, managedLogin: scope.managed, methodIndex: scope.methodIndex,
        manualCallback: !scope.managed && next.manualCallback === true, callbackInput,
        instructions: loginText(next.instructions), hint: loginText(next.hint), userCode: textOf(next.userCode),
        ...(authUrl ? { authUrl, ...(browserOpened !== undefined ? { browserOpened } : {}) } : {}),
        ...(scope.managed ? { phase: next.phase, saved: next.saved, remainingMs: next.remainingMs } : {}),
      }
      if (savingUnconfirmed(safe)) { clearCallback(); setValues({}) }
      else if (progressRef.current.state !== safe.state || progressRef.current.callbackInput !== safe.callbackInput) clearCallback()
      progressRef.current = safe
      setProgress(safe)
    }
    const bridge = shared().workbuddyDesktop
    const unsubscribeState = bridge?.onLoginState?.(receiveProgress)
    const unsubscribeProgress = bridge?.onLoginProgress?.(receiveProgress)
    const guard = () => {
      const scope = loginScope.current
      if (scope && (!sameServer(scope) || !accountProviderAvailable(provider))) {
        scope.cancelled = true; scope.cancelSent = true; scope.cleanup?.()
        loginScope.current = null
        progressRef.current = {}; setProgress({}); clearCallback()
        snapshot.current = { ...snapshot.current, inputs: {}, proxyDraft: { ...snapshot.current.proxyDraft, password: '' } }
        setValues({}); setUploads([]); setImportProgress(null); setBatchFailures([]); setProxyDraft({ ...snapshot.current.proxyDraft, password: '' })
      }
    }
    const unsubscribeBoot = subscribePluginStartup(guard)
    const unsubscribeStateRefresh = shared().wbApp?.subscribeRefresh?.(guard)
    const pagehide = () => { void cancelOwnedLogin(true).catch(() => {}); alive.current = false; progressRef.current = {}; setProgress({}) }
    const pageshow = () => { alive.current = true }
    window.addEventListener(MANAGEMENT_SESSION_CHANGE, guard)
    window.addEventListener('pagehide', pagehide)
    window.addEventListener('pageshow', pageshow)
    return () => {
      alive.current = false
      void cancelOwnedLogin(true).catch(() => {})
      clearCallback()
      progressRef.current = {}
      controllerRef.current?.dispose()
      controllerRef.current = null
      if (typeof unsubscribeState === 'function') unsubscribeState()
      if (typeof unsubscribeProgress === 'function') unsubscribeProgress()
      unsubscribeBoot(); unsubscribeStateRefresh?.()
      window.removeEventListener(MANAGEMENT_SESSION_CHANGE, guard)
      window.removeEventListener('pagehide', pagehide)
      window.removeEventListener('pageshow', pageshow)
    }
  }, [provider, prefix, preserveOnHide])

  React.useEffect(() => {
    const scope = loginScope.current
    if (!progress.managedLogin || !scope || !Number.isFinite(scope.deadline)) return
    const timer = window.setTimeout(() => {
      if (loginScope.current !== scope || !sameServer(scope)) return
      // 托管到期只清输入；保存进度可能尚未到达，不能自动取消并丢弃迟到真实回执。
      clearCallback(); setValues({})
    }, Math.max(0, scope.deadline - performance.now()))
    return () => window.clearTimeout(timer)
  }, [progress.state, progress.managedLogin, progress.phase, progress.saved, progress.remainingMs])

  async function finish(result: LoginResult, extraWarning?: string): Promise<void> {
    clearCallback()
    progressRef.current = {}
    loginScope.current = null
    if (!alive.current) { await shared().wbApp?.refresh?.(); return }
    setProgress({})
    setValues({})
    setName('')
    setUploads([])
    setImportProgress(null)
    setBatchFailures([])
    settings.resetAll()
    setError('')
    const nickname = textOf(result.session?.nickname)
    const combinedWarning = [result.warning, extraWarning].filter(Boolean).join('\n')
    const mergedResult = combinedWarning ? { ...result, warning: combinedWarning } : result
    await afterAdd(addedLabelOf(result.account) || nickname, label, mergedResult, provider)
    if (combinedWarning) toast(t('账号已添加，但请留意：{warning}', { warning: combinedWarning }), 'warn')
  }

  async function submit(): Promise<void> {
    if (!method || submitLocked || savingRef.current || requestingRef.current || methodsState.phase === 'loading') return
    try {
      if (!accountProviderAvailable(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
      if (method.reason) throw new Error(method.reason)
      const fields = readInputs(credentialInputMethod(method, uploads), inputs)
      let proxyPayload
      try {
        proxyPayload = readProxyDraft(proxyDraft)
      } catch (failure) {
        settings.setPane('network')
        throw failure
      }
      if (proxyPayload && 'source' in proxyPayload && proxyPayload.source === 'rotate') {
        settings.setPane('network')
        throw new Error(t('登录代理不支持轮询出网，请选择一条已保存的代理'))
      }
      const settingsResult = settings.readSettingsPayload({ supportsUsage, uaEditable })
      if ('error' in settingsResult) {
        settings.setPane(settingsResult.pane)
        throw new Error(settingsResult.error)
      }
      const fileField = method.file?.field
      const { items: batchItems, pastedExport } = resolveBatchImportItems(method, uploads, fields)
      const isBatch = batchItems.length > 1 || batchItems.some(item => item.exportKind === 'multi')
      if (isBatch) {
        const otherFields = Object.fromEntries(Object.entries(fields).filter(([key]) => key !== fileField && key !== 'name'))
        const wasFirstAccount = isProviderFirstAccount(provider)
        savingRef.current = true
        setSaving(true)
        proxy.setBusy(true)
        setError('')
        setBatchFailures([])
        setImportProgress(null)
        const outcome = await runCredentialBatchImport({
          provider, method, items: batchItems, otherFields,
          baseSettingsPayload: settingsResult.payload, proxyPayload,
          exportProxyFields: proxy.importFields(),
          isCancelled: () => !alive.current,
          onProgress: next => { if (alive.current) setImportProgress(next) },
        })
        if (!alive.current) {
          if (outcome.added + outcome.updated > 0) await shared().wbApp?.refresh?.()
          return
        }
        await finalizeBatchImport({
          provider, label, wasFirstAccount, itemsCount: batchItems.length, uploads, outcome,
          onClearAll: () => {
            setValues({}); setUploads([]); setName(''); setBatchFailures([]); setImportProgress(null)
            settings.resetAll(); setError('')
          },
          onPartialFailure: (remaining, failures) => {
            setUploads(remaining)
            if (fileField) {
              setValues(previous => ({
                ...previous,
                [method.id]: { ...previous[method.id], [fileField]: remaining.length === 1 && remaining[0].text !== undefined ? remaining[0].text : '' },
              }))
            }
            setBatchFailures(failures)
          },
          onAllFailed: (errorText, failures) => {
            if (errorText) setError(errorText)
            setBatchFailures(failures)
          },
        })
        return
      }
      const remark = method.prompts.some(prompt => prompt.key === 'name') ? fields.name : name.trim()
      const singleUpload = uploads[0]
      const isSingleExport = Boolean(singleUpload?.exportKind === 'single' || pastedExport?.kind === 'single')
      let payload: Record<string, unknown>
      if (method.type === 'api') {
        if (singleUpload) {
          const otherFields = Object.fromEntries(Object.entries(fields).filter(([key]) => key !== fileField))
          payload = buildCredentialAccountPayload({
            provider,
            method,
            item: singleUpload,
            otherFields,
            baseSettingsPayload: settingsResult.payload,
            proxyPayload,
            exportProxyFields: proxy.importFields(),
            remark: remark || undefined,
          })
        } else {
          payload = {
            provider,
            ...(isSingleExport ? proxy.importFields() : { proxy: proxyPayload }),
            ...(remark ? { name: remark } : {}),
            ...settingsResult.payload,
            methodIndex: method.methodIndex,
            inputs: fields,
            ...credentialFilePayloadForItem(method, undefined, fields),
          }
        }
      } else if (method.type === 'desktop') {
        payload = {
          provider,
          proxy: proxyPayload,
          ...(remark ? { name: remark } : {}),
          ...settingsResult.payload,
          importDesktop: true,
        }
      } else {
        throw new Error(t('此登录方式需要由共享登录桥发起'))
      }
      savingRef.current = true
      setSaving(true)
      proxy.setBusy(true)
      setError('')
      setBatchFailures([])
      // API 方法必须进入 account.normalize，不能把 Key/Token 交给 OAuth start。
      const rawResult = await postAccount(payload)
      const normalized = normalizeLoginResult(rawResult)
      const result = isSingleExport ? normalized : await saveAccountProxySelection(normalized, proxyPayload)
      await finish(result)
    } catch (failure) {
      if (alive.current) setError(describeError(failure), failure)
    } finally {
      savingRef.current = false
      proxy.setBusy(false)
      if (alive.current) {
        setSaving(false)
        setImportProgress(null)
      }
    }
  }

  function selectMethod(value: string): void {
    clearCallback()
    progressRef.current = {}
    setValues({})
    setUploads([])
    setImportProgress(null)
    setBatchFailures([])
    setSelected(value)
    setMode(defaultBrowserMode)
    setError('')
    setProgress({})
  }

  async function openExternalAuthUrl(url: string): Promise<void> {
    const valid = safeLoginUrl(url)
    if (!valid) return
    if (!isDesktop) {
      const popup = window.open('about:blank', '_blank')
      if (!popup) {
        toast(t('浏览器拦截了新标签页，请复制上方链接手动打开'), 'warn')
        return
      }
      popup.opener = null
      popup.location.replace(valid)
      return
    }
    await openExternal(valid)
  }

  function startFlow(): void {
    if (submitLocked || !method || requestingRef.current || methodsState.phase === 'loading') return
    try {
      if (!accountProviderAvailable(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
      readInputs(method, inputs)
      try {
        const proxyPayload = readProxyDraft(proxyDraft)
        if (proxyPayload && 'source' in proxyPayload && proxyPayload.source === 'rotate') {
          throw new Error(t('登录代理不支持轮询出网，请选择一条已保存的代理'))
        }
      } catch (failure) {
        settings.setPane('network')
        throw failure
      }
      const settingsCheck = settings.readSettingsPayload({ supportsUsage, uaEditable })
      if ('error' in settingsCheck) {
        settings.setPane(settingsCheck.pane)
        throw new Error(settingsCheck.error)
      }
      if (!controllerRef.current) throw new Error(t('共享登录桥不可用，请重新打开面板'))
      controllerRef.current.start()
    } catch (failure) { setError(describeError(failure), failure) }
  }

  async function startSmsFlow(): Promise<boolean> {
    if (submitLocked || !method || requestingRef.current || methodsState.phase === 'loading') {
      throw new Error(loginView.hint || t('正在处理本次登录，请勿重复提交。'))
    }
    if (!accountProviderAvailable(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
    readInputs(method, inputs)
    try {
      readProxyDraft(proxyDraft)
    } catch (failure) {
      settings.setPane('network')
      throw failure
    }
    const settingsCheck = settings.readSettingsPayload({ supportsUsage, uaEditable })
    if ('error' in settingsCheck) {
      settings.setPane(settingsCheck.pane)
      throw new Error(settingsCheck.error)
    }
    if (!controllerRef.current) throw new Error(t('共享登录桥不可用，请重新打开面板'))
    if (shared().wbWebLogin?.isActive?.()) throw new Error(loginView.hint || t('此提供商已有登录任务，请先完成或取消。'))
    setError('')
    await (controllerRef.current.start() as unknown as Promise<unknown>)
    return false
  }

  function cancelFlow(): void {
    clearCallback()
    setValues({})
    setUploads([])
    setImportProgress(null)
    setBatchFailures([])
    setProxyDraft({ ...snapshot.current.proxyDraft, password: '' })
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
      if (!accountProviderAvailable(provider)) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
      if (!engine?.isActive?.() || engine.activeProvider?.() !== provider) throw new Error()
      const bridge = shared().workbuddyDesktop
      if (!bridge?.submitLoginCallback) throw new Error()
      // URL / 裸 code 只在输入框与这次受保护 POST 中存在，不 trim、解析或伪造 URL。
      const reply = objectOf(await bridge.submitLoginCallback(flow.state, raw))
      if (!isCurrent()) return
      if (reply?.success === false || reply?.ok === false || reply?.error) throw new Error()
      if (reply?.retryable === true) {
        clearCallback()
        setCallbackError(loginText(reply.hint) || t('回调未被接受，请重新复制本次登录的完整回调地址'))
        return
      }
      const nextUrl = textOf(reply?.nextUrl)
      if (nextUrl) {
        const parsed = new URL(nextUrl)
        if (parsed.protocol !== 'https:' || parsed.username || parsed.password || /[\u0000-\u0020\u007f]/.test(nextUrl)) throw new Error()
        clearCallback()
        setNextAuthUrl(nextUrl)
        setCallbackNotice(t('还需继续授权，请打开下一步授权页，完成后回填新的完整回调地址。'))
        return
      }
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

  const optionalName = method?.prompts.find(prompt => prompt.key === 'name' && prompt.type === 'text' && !prompt.secret && prompt.optional && !prompt.when)
  const formPrompts = method?.prompts.filter(prompt => prompt !== optionalName) || []
  const accountName = optionalName ? inputs[optionalName.key] ?? optionalName.default ?? '' : name
  const pastedExport = !uploads.length && method?.file ? inspectSoftwareExport(inputs[method.file.field] || '') : undefined
  const isMultiImport = uploads.length > 1 || uploads.some(item => item.exportKind === 'multi') || pastedExport?.kind === 'multi'
  const submitLabel = saving
    ? (importProgress && importProgress.total > 1
      ? t('导入中 ({current}/{total})…', { current: importProgress.current, total: importProgress.total })
      : t('添加中…'))
    : uploads.length > 1
      ? t('导入 {count} 个文件', { count: uploads.length })
      : t('添加账号')
  const instructions = mergeLoginInstructions(methodsState.instructions, progress.managedLogin ? '' : method?.instructions,
    method?.browserMode === 'plugin' && !method.embeddedOnly ? pluginBrowserInstructions(provider) : '',
    loginText(progress.instructions), loginText(progress.hint), loginText(loginView.hint))
  const proxyLocked = locked || loginView.waiting
  const optionsLocked = method?.type === 'sms' ? saving || proxy.busy : proxyLocked
  const owned = loginScope.current
  const managed: ManagedLoginScope | undefined = progress.managedLogin && owned && owned.methodIndex !== undefined ? {
    provider, methodIndex: owned.methodIndex, key: JSON.stringify([owned.origin, owned.bootId, owned.managementGeneration, provider, owned.state]),
    deadline: owned.deadline, current: () => alive.current && scopeCurrent(owned) && progressRef.current.state === owned.state && !savingUnconfirmed(progressRef.current),
  } : undefined

  return <div className='add-provider-block plugin-account-form' hidden={!active}>
    {methodsState.phase === 'loading' ? <p role='status'><Spinner /> {t('正在读取插件登录方式…')}</p> : null}
    {methodsState.reason ? <Alert variant={methodsState.phase === 'error' ? 'destructive' : 'warning'}>
      <AlertTitle>{methodsState.phase === 'error' ? t('无法读取登录方式') : t('登录方式说明')}</AlertTitle>
      <AlertDescription className='whitespace-pre-wrap break-words'>{methodsState.reason}</AlertDescription>
    </Alert> : null}
    {methodsState.phase !== 'loading' && (method || localAction) ? <PluginLoginMethodPicker
      label={t('{label} 账号的添加方式', { label })} disabled={locked || loginView.waiting}
      value={localActionSelected ? localAction : method?.id || ''} methods={methodsState.methods} onChange={selectMethod}
      extra={localAction ? [{ value: localAction, label: t('本机导入') }] : []}
    /> : null}

    {localActionsAvailable && provider === 'chatglm' ? <PluginDesktopImport
      active={active && localActionSelected} disabled={locked || loginView.waiting} source={shared().workbuddyDesktop}
      scopeIdentity={JSON.stringify([window.location.origin, bootId(), availability])} footerHost={footerHost} preserveOnHide
      current={() => shared().workbuddyDesktop?.platform === 'windows' && Boolean(shared().__TAURI_INTERNALS__?.invoke) && accountProviderAvailable('chatglm')}
      request={desktopImportRequest} onSaved={async importedAccount => {
        const extraWarning = await settings.applySettingsAfterCreate(importedAccount?.id, provider, { supportsUsage, uaEditable })
        settings.resetAll()
        await afterAdd('', label, {
          account: importedAccount || undefined,
          ...(extraWarning ? { warning: extraWarning } : {}),
        }, provider)
        if (extraWarning) toast(extraWarning, 'warn')
      }} t={t} /> : null}
    {(progress.active && progress.state) || (methodAvailable && method.type === 'sms' && Boolean(method.sms) && !method.reason)
      ? <PluginLoginInteraction
        key={methodAvailable && method.type === 'sms' ? `sms:${provider}:${method.id}` : progress.state}
        flow={progress.active && progress.state ? progress.state : ''} t={t}
        smsConfig={methodAvailable && method.type === 'sms' && !method.reason ? method.sms : undefined}
        onStartSms={startSmsFlow} startingSms={loginView.waiting}
        smsStartDisabled={submitLocked || requestingRef.current || methodsState.phase === 'loading' || Boolean(method?.reason)}
        remark={accountName} onRemarkChange={setName}
        openExternal={managed ? async url => {
          if (!managed.current()) throw new DOMException('请求已取消', 'AbortError')
          if (owned?.bridge?.platform === 'web') {
            const popup = window.open('about:blank', '_blank')
            if (!popup) throw new Error('popup_blocked')
            popup.opener = null
            popup.location.replace(url)
            return
          }
          if (!owned?.bridge?.openReleasePage) throw new Error('external_browser_unavailable')
          await owned.bridge.openReleasePage(url)
        } : openExternal}
        managed={managed} openEmbedded={async (flow, interaction) => {
          if (!managed?.current() || !owned?.bridge?.openManagedLogin) throw new DOMException('请求已取消', 'AbortError')
          await owned.bridge.openManagedLogin(flow, interaction)
        }} request={interactionRequest} /> : null}
    {method && methodsState.phase !== 'loading' && instructions ? <p className='plugin-login-instructions'>{instructions}</p> : null}
    {progress.userCode ? <p className='plugin-login-local-scope'>{t('设备授权码：{code}', { code: progress.userCode })}</p> : null}

    {method && methodsState.phase !== 'loading' ? <section className='plugin-login-content'>
      {method.localOnly && !method.desktopOnly && !method.embeddedOnly ? <Alert variant='warning'>
        <AlertTitle>{t('需要网关本机环境')}</AlertTitle>
        <AlertDescription>{t('“本机”指运行网关的机器，不是打开面板的电脑。远程浏览器也可以发起，但网关本机可能需要安装客户端并已登录；是否可用由网关返回结果决定。')}</AlertDescription>
      </Alert> : null}
      {method.manualCallback && manualWaiting ? <p className='plugin-login-note'>{method.callbackInput === 'code'
        ? t('此方式需要原样提交授权码或令牌；输入会隐藏，不会当作 URL 或 API 凭证解析。提交后仍需等待网关确认。')
        : t('此方式需要完整回调地址；请原样粘贴，不拆分参数或重建地址。提交后仍需等待网关确认。')}</p> : null}
      {resolvedPrompts.error ? <Alert variant='destructive'><AlertDescription>{resolvedPrompts.error}</AlertDescription></Alert> : null}
      {method.reason ? <Alert variant='warning'><AlertDescription>{method.reason}</AlertDescription></Alert> : null}
      <PluginLoginBrowser method={method} provider={provider} desktop={isDesktop}
        value={mode} onChange={setMode} disabled={locked || loginView.waiting} />
      {(usesExternalBrowser || Boolean(loginView.waiting && progress.active && progress.provider === provider && progress.authUrl)) && !progress.managedLogin ? (
        <PluginLoginExternalLink
          id={`${prefix}-auth-url`}
          url={loginView.waiting && progress.active && progress.provider === provider ? (progress.authUrl || '') : ''}
          browserOpened={loginView.waiting && progress.active && progress.provider === provider && progress.authUrl ? progress.browserOpened : undefined}
          disabled={locked || loginView.waiting}
          noAutoOpen={noAutoOpen}
          onNoAutoOpenChange={setNoAutoOpen}
          onOpenExternal={openExternalAuthUrl}
          onCopied={ok => toast(ok ? t('授权链接已复制') : t('请手动复制选中的链接'), ok ? 'ok' : 'warn')}
          t={t}
        />
      ) : null}
      <PluginLoginFields method={method} prompts={formPrompts} values={inputs} prefix={prefix} disabled={locked || loginView.waiting}
        uploads={uploads} onUploads={next => { setUploads(next); setBatchFailures([]); setError('') }} onReading={setReadingFile}
        onChange={(key, value) => {
          setBatchFailures([])
          setValues(previous => ({ ...previous, [method.id]: { ...previous[method.id], [key]: value } }))
        }} />
      {method.type === 'oauth' || method.type === 'sms' ? <AddFoot host={footerHost} active={active}>
        <Button id={`${prefix}-start`} disabled={submitLocked || Boolean(method.reason)} onClick={startFlow}
          hidden={method.type === 'sms'}>
          {loginView.waiting ? <Spinner /> : null}{method.type === 'sms' ? t('开始短信登录') : loginView.label}
        </Button>
        <Button id={`${prefix}-cancel`} variant='outline' hidden={!loginView.waiting}
          onClick={cancelFlow}>{t('取消等待')}</Button>
      </AddFoot> : <AddFoot host={footerHost} active={active}>
        <Button disabled={submitLocked || Boolean(method.reason)}
          onClick={() => void submit()}>{saving ? <Spinner /> : null}{submitLabel}</Button>
      </AddFoot>}
      {manualWaiting ? <div className='flex flex-col gap-2'>
        {nextAuthUrl ? <Button variant='outline' className='self-start' disabled={callbackBusy}
          onClick={() => {
            if (progressRef.current.active && progressRef.current.provider === provider) void openExternal(nextAuthUrl)
          }}>{t('打开下一步授权页')}</Button> : null}
        <Label htmlFor={`${prefix}-callback`}>{callbackType === 'code'
          ? t('授权码或令牌（原样提交）') : t('完整回调地址（原样提交）')}</Label>
        <Input key={`${progress.state}:${callbackType}`} id={`${prefix}-callback`} ref={callbackInputRef}
          type='password' defaultValue=''
          autoComplete='new-password' spellCheck={false} autoCapitalize='none'
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
      {error ? <Alert variant='destructive'><AlertTitle>{diagnostic ? t('登录失败') : t('添加失败')}</AlertTitle>
        <AlertDescription className='whitespace-pre-wrap break-words'>{error}</AlertDescription>
        <PluginLoginDiagnostic diagnostic={diagnostic} />
      </Alert> : null}
      {batchFailures.length ? <Alert variant='destructive'>
        <AlertTitle>{t('部分项目导入失败（{count}）', { count: batchFailures.length })}</AlertTitle>
        <AlertDescription>
          <ul className='plugin-credential-failures'>
            {batchFailures.slice(0, 6).map((item, idx) => (
              <li key={`${item.label}:${idx}`}>{item.label}：{item.reason}</li>
            ))}
          </ul>
          {batchFailures.length > 6 ? <p className='mt-1 text-xs'>{t('其余 {n} 项失败未展开', { n: batchFailures.length - 6 })}</p> : null}
        </AlertDescription>
      </Alert> : null}
    </section> : null}
    {error && !methodAvailable ? <Alert variant='destructive'><AlertDescription>{error}</AlertDescription>
      <PluginLoginDiagnostic diagnostic={diagnostic} />
    </Alert> : null}
    {methodAvailable && !isMultiImport && (optionalName || !method.prompts.some(prompt => prompt.key === 'name')) ? (
      <SettingGroup title={t('账号备注')}>
        <div className='acct-dlg-block plugin-login-field'>
          <Label htmlFor={`${prefix}-name`}>{t('备注名')}<span className='plugin-login-optional'>{t('可选')}</span></Label>
          <Input id={`${prefix}-name`} maxLength={MAX_NAME_LENGTH} placeholder={t('可选，留空使用账号昵称')}
            value={accountName} disabled={optionsLocked} onChange={event => {
              const value = event.currentTarget.value
              if (optionalName) setValues(previous => ({ ...previous, [method.id]: { ...previous[method.id], [optionalName.key]: value } }))
              else setName(value)
            }} />
        </div>
      </SettingGroup>
    ) : null}
    <PluginLoginSecurity />
  </div>
}
