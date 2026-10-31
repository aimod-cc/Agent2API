import * as React from 'react'
import { createPortal } from 'react-dom'
import {
  Alert, AlertDescription, Button, Collapsible, CollapsibleContent, CollapsibleTrigger,
  Input, InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput, Label,
  SegmentedControl, Spinner,
} from '@ui'
import { t } from '../i18n'
import { afterAdd, addedLabelOf, describeError, normalizeLoginResult, postAccount, runtimeProviderOf, shared, startLogin, toast } from './add-account-bridge'
import { draftOfProxy, ProxyForm, readProxyDraft, type ProxyDraft } from './accounts-proxy-form'
import type { WebLoginController, WebLoginView } from './add-account-bridge'

function LockIcon() {
  return <svg viewBox='0 0 24 24' aria-hidden='true'><rect x='5' y='10' width='14' height='11' rx='3' /><path d='M8 10V7a4 4 0 0 1 8 0v3' /></svg>
}

function GlobeIcon() {
  return <svg viewBox='0 0 24 24' aria-hidden='true'><circle cx='12' cy='12' r='9' /><ellipse cx='12' cy='12' rx='4' ry='9' /><path d='M3 12h18' /></svg>
}

function ChevronIcon() {
  return <svg viewBox='0 0 24 24' aria-hidden='true'><path d='m9 5 7 7-7 7' /></svg>
}

function CredentialInput({ id, label, optional, placeholder, value, onChange, disabled, maxLength, children }: {
  id: string; label: string; optional?: boolean; placeholder: string; value: string
  onChange: (value: string) => void; disabled: boolean; maxLength: number; children?: React.ReactNode
}) {
  const [visible, setVisible] = React.useState(false)
  return <div className='compact-login-field'>
    <div className='compact-login-label'><Label htmlFor={id}>{label}{optional ? <span>{t('可选')}</span> : null}</Label>{children}</div>
    <InputGroup>
      <InputGroupInput id={id} type={visible ? 'text' : 'password'} value={value} disabled={disabled}
        maxLength={maxLength} placeholder={placeholder} autoComplete='new-password' spellCheck={false}
        onChange={event => onChange(event.currentTarget.value)} />
      <InputGroupAddon align='inline-end'><InputGroupButton size='icon-xs' disabled={disabled}
        aria-label={visible ? t('隐藏凭证') : t('显示凭证')} onClick={() => setVisible(previous => !previous)}>
        <svg viewBox='0 0 24 24' aria-hidden='true'><path d='M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12Z' /><circle cx='12' cy='12' r='3' /></svg>
      </InputGroupButton></InputGroupAddon>
    </InputGroup>
  </div>
}

function proxyLabel(draft: ProxyDraft): string {
  if (draft.mode === 'none') return t('直连')
  if (draft.mode === 'pool') return t('已保存的代理')
  if (draft.mode === 'custom') return draft.host ? `${draft.host}:${draft.port}` : t('手动代理')
  return t('请选择代理')
}

export function DeepseekAccountBlock({ active, footerHost, onClose }: {
  active: boolean; footerHost: HTMLElement | null; onClose: () => void
}): React.ReactElement {
  const id = `${React.useId()}-deepseek-login`
  const desktop = shared().workbuddyDesktop?.platform !== 'web'
  const [method, setMethod] = React.useState<'web' | 'token'>(() => desktop ? 'web' : 'token')
  const [token, setToken] = React.useState('')
  const [cookie, setCookie] = React.useState('')
  const [name, setName] = React.useState('')
  const [proxy, setProxy] = React.useState(() => draftOfProxy(undefined))
  const [saving, setSaving] = React.useState(false)
  const [error, setError] = React.useState('')
  const [guideOpen, setGuideOpen] = React.useState(false)
  const [proxyOpen, setProxyOpen] = React.useState(false)
  const [view, setView] = React.useState<WebLoginView>({ label: t('打开网页登录'), hint: '', disabled: false, waiting: false })
  const controller = React.useRef<WebLoginController | null>(null)
  const alive = React.useRef(false)
  const savingRef = React.useRef(false)
  const snapshot = React.useRef({ name, proxy })
  snapshot.current = { name, proxy }
  const locked = saving || view.disabled
  const tokenValid = Boolean(token.trim()) && !token.trim().startsWith('sk-') && !/[\r\n]/.test(token)

  React.useEffect(() => {
    alive.current = true
    controller.current = shared().wbWebLogin?.create({
      provider: 'deepseek-web', buttonId: `${id}-start`, cancelId: `${id}-cancel`, hintId: `${id}-hint`,
      busyText: t('等待登录完成…'), onStateChange: setView,
      texts: () => ({ button: t('打开网页登录'), hint: '' }),
      start: async () => {
        const current = snapshot.current
        if (!runtimeProviderOf('deepseek-web')) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
        try {
          return await startLogin('deepseek-web', '', 'embedded', false, {
            methodIndex: 1, inputs: current.name.trim() ? { name: current.name.trim() } : {}, proxy: readProxyDraft(current.proxy),
          })
        } catch (failure) {
          if (alive.current) setError(describeError(failure))
          throw failure
        }
      },
      onSuccess: async raw => {
        const result = normalizeLoginResult(raw)
        if (!result.ok || result.canceled || !(result.account?.id || result.session?.accountUid || result.session?.accountId)) {
          throw new Error(t('登录桥未返回已完成的账号或会话，请重试'))
        }
        await afterAdd(addedLabelOf(result.account), t('DeepSeek 网页'))
        if (result.warning) toast(result.warning, 'warn')
      },
    }) || null
    return () => {
      alive.current = false
      controller.current?.dispose()
      controller.current = null
      void shared().wbWebLogin?.cancelIfActive?.('deepseek-web')
    }
  }, [id])

  React.useEffect(() => {
    if (active) return
    setToken('')
    setCookie('')
    setError('')
    setProxy(previous => ({ ...previous, password: '' }))
    void shared().wbWebLogin?.cancelIfActive?.('deepseek-web')
  }, [active])

  async function addToken(): Promise<void> {
    if (!tokenValid || locked || savingRef.current) return
    try {
      if (!runtimeProviderOf('deepseek-web')) throw new Error(t('该提供商已不在运行时目录中，请返回列表重新选择'))
      const selectedProxy = readProxyDraft(proxy)
      savingRef.current = true
      setSaving(true)
      setError('')
      const result = normalizeLoginResult(await postAccount({
        provider: 'deepseek-web', accessToken: token.trim(), cookie: cookie.trim(), name: name.trim() || undefined, proxy: selectedProxy,
      }))
      if (!result.account?.id) throw new Error(t('登录桥未返回已完成的账号或会话，请重试'))
      if (alive.current) { setToken(''); setCookie('') }
      await afterAdd(addedLabelOf(result.account), t('DeepSeek 网页'))
      if (result.warning) toast(result.warning, 'warn')
    } catch (failure) {
      if (alive.current) setError(describeError(failure))
    } finally {
      savingRef.current = false
      if (alive.current) setSaving(false)
    }
  }

  function start(): void {
    if (locked) return
    setError('')
    try {
      readProxyDraft(proxy)
      if (!controller.current) throw new Error(t('共享登录桥不可用，请重新打开面板'))
      controller.current.start()
    } catch (failure) { setError(describeError(failure)) }
  }

  function changeMethod(next: 'web' | 'token'): void {
    setMethod(next)
    setToken('')
    setCookie('')
    setError('')
  }

  const footer = <>
    <span className='compact-login-security'><LockIcon />{t('凭证仅发送至当前网关')}</span>
    <div className='compact-login-actions'>
      <Button id={`${id}-cancel`} variant='outline' disabled={saving} onClick={() => {
        if (view.waiting) controller.current?.cancel()
        else onClose()
      }}>{view.waiting ? t('取消等待') : t('取消')}</Button>
      {method === 'web' ? <Button id={`${id}-start`} disabled={locked} onClick={start}>
        {view.waiting ? <Spinner /> : <GlobeIcon />}{view.label}
      </Button> : <Button disabled={locked || !tokenValid} onClick={() => void addToken()}>
        {saving ? <Spinner /> : null}{saving ? t('添加中…') : t('添加账号')}
      </Button>}
    </div>
  </>

  return <div className='compact-account-login' hidden={!active}>
    {desktop ? <SegmentedControl className='compact-login-methods' value={method} disabled={locked}
      aria-label={t('添加方式')} options={[
        { value: 'web', label: <>{t('网页登录')}<span className='compact-login-recommend'>{t('推荐')}</span></> },
        { value: 'token', label: t('粘贴令牌') },
      ]} onValueChange={changeMethod} /> : null}
    {method === 'web' ? <div className='compact-login-description'>
      {view.waiting ? <p role='status'>{t('等待你在官方页面完成登录，成功后自动添加账号。')}</p>
        : <p>{t('在 DeepSeek 官网完成登录，自动加入账号列表。')}<br /><span>{t('无需手动复制令牌，验证码在官方页面完成。')}</span></p>}
    </div> : <>
      <CredentialInput id={`${id}-token`} label={t('网页访问令牌')} placeholder={t('粘贴 userToken.value')}
        value={token} onChange={setToken} disabled={locked} maxLength={16384}>
        <Button variant='link' size='xs' onClick={() => setGuideOpen(previous => !previous)} aria-expanded={guideOpen}>
          {t('如何获取')}
        </Button>
      </CredentialInput>
      <p className='compact-login-hint'>{t('使用网页令牌，不是收费 API Key。')}</p>
      {guideOpen ? <p className='compact-login-guide'>{t('登录 chat.deepseek.com 后，打开开发者工具 → Application → Local Storage，复制 userToken 的 value。')}</p> : null}
      {token.trim().startsWith('sk-') ? <p className='compact-login-error'>{t('请使用网页访问令牌，不要填写 sk- 开头的 API Key。')}</p> : null}
      <CredentialInput id={`${id}-cookie`} label='Cookie' optional placeholder={t('需要时填写完整 Cookie')}
        value={cookie} onChange={setCookie} disabled={locked} maxLength={32768} />
      {!desktop ? <p className='compact-login-hint'>{t('浏览器面板无法读取其他站点的登录态，请使用本人网页账号的令牌。')}</p> : null}
    </>}
    <div className='compact-login-field'>
      <Label htmlFor={`${id}-name`}>{t('备注名')}<span>{t('可选')}</span></Label>
      <Input id={`${id}-name`} value={name} disabled={locked} maxLength={100}
        placeholder={t('留空使用账号默认名称')} onChange={event => setName(event.currentTarget.value)} />
    </div>
    <Collapsible open={proxyOpen} onOpenChange={setProxyOpen} className='compact-login-proxy'>
      <CollapsibleTrigger className='compact-login-proxy-trigger' disabled={locked}>
        <span>{t('出网代理')}</span><span className='compact-login-proxy-summary'>{proxyLabel(proxy)}</span><ChevronIcon />
      </CollapsibleTrigger>
      <CollapsibleContent><div className='compact-login-proxy-body' inert={locked}>
        <ProxyForm draft={proxy} onChange={setProxy} idPrefix={`${id}-proxy`} />
      </div></CollapsibleContent>
    </Collapsible>
    {view.hint && !view.waiting ? <p id={`${id}-hint`} className='compact-login-hint'>{view.hint}</p> : null}
    {error ? <Alert variant='destructive'><AlertDescription>{error}</AlertDescription></Alert> : null}
    {footerHost && active ? createPortal(footer, footerHost) : null}
  </div>
}
