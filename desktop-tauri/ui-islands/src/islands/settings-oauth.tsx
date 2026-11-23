import * as React from 'react'
import {
  Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Input, Label, Switch, Textarea,
} from '@ui'
import { errorMessage, openExternal, shared, toast } from './settings-model'
import { t } from '../i18n'

export type BoundOauthAccount = {
  id: string
  login?: string
  email?: string
  name?: string
  boundAt?: number
}

export type PanelOauthSettingsData = {
  publicBaseUrl: string
  callbackPath: string
  google: {
    enabled: boolean
    clientId: string
    clientSecretSet: boolean
    allowedEmails: string[]
    boundAccounts: BoundOauthAccount[]
  }
  github: {
    enabled: boolean
    clientId: string
    clientSecretSet: boolean
    allowedUsers: string[]
    boundAccounts: BoundOauthAccount[]
  }
}

export function OauthSettingsPanel() {
  const [saved, setSaved] = React.useState<PanelOauthSettingsData | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [bindingProvider, setBindingProvider] = React.useState<'google' | 'github' | null>(null)
  const [error, setError] = React.useState('')

  const [publicBaseUrl, setPublicBaseUrl] = React.useState('')
  const [backendPort, setBackendPort] = React.useState<number>(3065)

  const [googleEnabled, setGoogleEnabled] = React.useState(false)
  const [googleClientId, setGoogleClientId] = React.useState('')
  const [googleClientSecret, setGoogleClientSecret] = React.useState('')
  const [googleEmails, setGoogleEmails] = React.useState('')

  const [githubEnabled, setGithubEnabled] = React.useState(false)
  const [githubClientId, setGithubClientId] = React.useState('')
  const [githubClientSecret, setGithubClientSecret] = React.useState('')
  const [githubUsers, setGithubUsers] = React.useState('')

  const pollTimerRef = React.useRef<number | null>(null)
  const prefix = React.useId()

  const stopPolling = React.useCallback(() => {
    if (pollTimerRef.current !== null) {
      window.clearInterval(pollTimerRef.current)
      pollTimerRef.current = null
    }
  }, [])

  React.useEffect(() => () => stopPolling(), [stopPolling])

  const accept = React.useCallback((data: PanelOauthSettingsData | null | undefined) => {
    if (!data || !data.google || !data.github) {
      throw new Error(t('未能读取第三方登录设置'))
    }
    setSaved(data)
    setPublicBaseUrl(data.publicBaseUrl || '')

    setGoogleEnabled(data.google.enabled === true)
    setGoogleClientId(data.google.clientId || '')
    setGoogleClientSecret('')
    setGoogleEmails((data.google.allowedEmails || []).join('\n'))

    setGithubEnabled(data.github.enabled === true)
    setGithubClientId(data.github.clientId || '')
    setGithubClientSecret('')
    setGithubUsers((data.github.allowedUsers || []).join('\n'))
  }, [])

  const load = React.useCallback(async () => {
    setBusy(true)
    setError('')
    try {
      const data = await shared().workbuddyDesktop?.getOauthSettings?.() as PanelOauthSettingsData | undefined
      accept(data)
    } catch (failure) {
      setError(errorMessage(failure))
    } finally {
      setBusy(false)
    }
  }, [accept])

  React.useEffect(() => {
    void load()
    const bridge = shared().workbuddyDesktop
    if (bridge?.platform !== 'web' && bridge?.getBackendStatus) {
      void bridge.getBackendStatus().then(status => {
        if (status?.port && Number.isInteger(status.port)) {
          setBackendPort(status.port)
        }
      }).catch(() => {})
    }
  }, [load])

  const resolvedBaseOrigin = React.useMemo(() => {
    const custom = publicBaseUrl.trim().replace(/\/+$/, '')
    if (custom) return custom
    const bridge = shared().workbuddyDesktop
    if (bridge?.platform && bridge.platform !== 'web') {
      return `http://127.0.0.1:${backendPort}`
    }
    if (typeof window !== 'undefined' && window.location.origin.startsWith('http')
      && !window.location.hostname.endsWith('tauri.localhost')) {
      return window.location.origin
    }
    return `http://127.0.0.1:${backendPort}`
  }, [publicBaseUrl, backendPort])

  const callbackUrl = `${resolvedBaseOrigin}/api/panel/oauth/callback`

  function buildFullPatch(): Record<string, unknown> {
    return {
      publicBaseUrl: publicBaseUrl.trim(),
      google: {
        enabled: googleEnabled,
        clientId: googleClientId.trim(),
        allowedEmails: googleEmails
          .split(/[\n,，;；]/)
          .map(s => s.trim())
          .filter(Boolean),
        ...(googleClientSecret.trim() ? { clientSecret: googleClientSecret.trim() } : {}),
      },
      github: {
        enabled: githubEnabled,
        clientId: githubClientId.trim(),
        allowedUsers: githubUsers
          .split(/[\n,，;；]/)
          .map(s => s.trim())
          .filter(Boolean),
        ...(githubClientSecret.trim() ? { clientSecret: githubClientSecret.trim() } : {}),
      },
    }
  }

  async function save(patch: Record<string, unknown>, successMsg = t('第三方登录设置已保存')) {
    setBusy(true)
    setError('')
    try {
      const result = await shared().workbuddyDesktop?.saveOauthSettings?.(patch) as PanelOauthSettingsData | undefined
      accept(result)
      toast(successMsg, 'ok')
    } catch (failure) {
      const msg = errorMessage(failure)
      setError(msg)
      toast(msg, 'err')
    } finally {
      setBusy(false)
    }
  }

  async function startBind(provider: 'google' | 'github') {
    stopPolling()
    setBindingProvider(provider)
    setError('')
    try {
      const res = await shared().workbuddyDesktop?.startOauthBind?.({
        provider,
        origin: resolvedBaseOrigin,
        patch: buildFullPatch(),
      }) as { state?: string; authorizeUrl?: string } | undefined
      if (!res?.state || !res?.authorizeUrl) {
        throw new Error(t('未能生成授权地址'))
      }
      const bridge = shared().workbuddyDesktop
      if (bridge?.platform && bridge.platform !== 'web') {
        await openExternal(res.authorizeUrl)
      } else {
        const popup = window.open(res.authorizeUrl, 'agent2api_oauth_bind', 'width=540,height=680')
        if (!popup) {
          await openExternal(res.authorizeUrl)
        }
      }
      const state = res.state
      const startedAt = Date.now()
      pollTimerRef.current = window.setInterval(() => {
        if (Date.now() - startedAt > 180_000) {
          stopPolling()
          setBindingProvider(null)
          return
        }
        void (async () => {
          try {
            const poll = await shared().workbuddyDesktop?.pollOauthState?.(state) as {
              status?: string
              identity?: string
              provider?: string
              error?: string
            } | undefined
            if (!poll || poll.status === 'pending') return
            stopPolling()
            setBindingProvider(null)
            if (poll.status === 'done') {
              await load()
              toast(
                t('已绑定 {provider} 账号：{id}', {
                  provider: poll.provider || (provider === 'google' ? 'Google' : 'GitHub'),
                  id: poll.identity || '',
                }),
                'ok',
              )
            } else if (poll.status === 'error') {
              const msg = poll.error || t('绑定失败')
              setError(msg)
              toast(msg, 'err')
            }
          } catch {
            // 轮询瞬时错误忽略
          }
        })()
      }, 1200)
    } catch (failure) {
      const msg = errorMessage(failure)
      setError(msg)
      toast(msg, 'err')
      setBindingProvider(null)
    }
  }

  async function copyCallbackUrl() {
    try {
      await navigator.clipboard.writeText(callbackUrl)
      toast(t('回调地址已复制'), 'ok')
    } catch {
      toast(callbackUrl, 'warn')
    }
  }

  const locked = busy || !saved

  return (
    <Card render={<section />} className='panel'>
      <CardHeader className='flex-row items-center justify-between gap-3'>
        <div className='flex items-center gap-2'>
          <CardTitle>{t('第三方登录 (OAuth)')}</CardTitle>
          {saved && (saved.google.enabled || saved.github.enabled) ? (
            <Badge variant='success'>{t('已启用')}</Badge>
          ) : (
            <Badge variant='outline'>{t('未启用')}</Badge>
          )}
        </div>
        <Button variant='ghost' size='sm' disabled={busy} onClick={() => void load()}>
          {t('刷新')}
        </Button>
      </CardHeader>

      <CardContent className='panel-body flex flex-col gap-4'>
        <div className='space-y-1.5'>
          <div className='flex items-center justify-between gap-2'>
            <Label htmlFor={`${prefix}-base`}>{t('面板外部访问地址（反代或自定义域名时填写，留空自动使用当前地址）')}</Label>
            <Button variant='outline' size='sm' onClick={() => void copyCallbackUrl()}>
              {t('复制回调地址')}
            </Button>
          </div>
          <Input
            id={`${prefix}-base`}
            value={publicBaseUrl}
            disabled={locked}
            placeholder='https://panel.example.com'
            onChange={e => setPublicBaseUrl(e.target.value)}
          />
          <p className='m-0 text-xs text-muted-foreground'>
            {t('请在 Google / GitHub OAuth 应用中登记回调地址：')}
            <code className='ml-1 select-all rounded bg-muted px-1.5 py-0.5 text-[11.5px]'>
              {callbackUrl}
            </code>
          </p>
        </div>

        {/* Google OAuth */}
        <div className='rounded-md border border-hairline bg-surface-2 p-3.5 flex flex-col gap-3'>
          <div className='flex items-center justify-between gap-3'>
            <div className='flex items-center gap-2'>
              <span className='text-[13.5px] font-semibold'>Google</span>
              <Badge variant={saved?.google.enabled ? 'success' : 'outline'}>
                {saved?.google.enabled ? t('已启用') : t('已关闭')}
              </Badge>
            </div>
            <Switch
              id={`${prefix}-g-enable`}
              checked={googleEnabled}
              disabled={locked}
              onCheckedChange={setGoogleEnabled}
            />
          </div>

          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-g-id`}>Client ID</Label>
              <Input
                id={`${prefix}-g-id`}
                value={googleClientId}
                disabled={locked}
                placeholder='xxxx.apps.googleusercontent.com'
                onChange={e => setGoogleClientId(e.target.value)}
              />
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-g-secret`}>Client Secret</Label>
              <Input
                id={`${prefix}-g-secret`}
                type='password'
                autoComplete='new-password'
                value={googleClientSecret}
                disabled={locked}
                placeholder={
                  saved?.google.clientSecretSet
                    ? t('已保存，留空则不修改')
                    : t('输入 Google Client Secret')
                }
                onChange={e => setGoogleClientSecret(e.target.value)}
              />
            </div>
          </div>

          <div className='space-y-1.5'>
            <div className='flex items-center justify-between gap-2'>
              <Label htmlFor={`${prefix}-g-emails`}>{t('允许登录的 Google 邮箱（每行一个，或点击右侧跳转绑定）')}</Label>
              <Button
                variant='outline'
                size='sm'
                disabled={
                  locked
                  || bindingProvider !== null
                  || !googleClientId.trim()
                  || (!googleClientSecret.trim() && !saved?.google.clientSecretSet)
                }
                onClick={() => void startBind('google')}
              >
                {bindingProvider === 'google' ? t('等待授权绑定…') : t('跳转绑定当前 Google 账号')}
              </Button>
            </div>
            <Textarea
              id={`${prefix}-g-emails`}
              rows={2}
              value={googleEmails}
              disabled={locked}
              placeholder='yourname@gmail.com'
              onChange={e => setGoogleEmails(e.target.value)}
            />
          </div>

          {saved?.google.boundAccounts && saved.google.boundAccounts.length > 0 ? (
            <div className='flex flex-wrap items-center gap-2'>
              <span className='text-xs text-muted-foreground'>{t('已绑定：')}</span>
              {saved.google.boundAccounts.map(acc => (
                <Badge key={acc.id} variant='outline' className='flex items-center gap-1.5'>
                  <span>{acc.email || acc.name || acc.id}</span>
                  <button
                    type='button'
                    className='ml-1 cursor-pointer border-0 bg-transparent p-0 text-muted-foreground hover:text-destructive'
                    disabled={locked}
                    onClick={() =>
                      void save({ google: { unbindId: acc.id } }, t('已解除 Google 账号绑定'))
                    }
                  >
                    ×
                  </button>
                </Badge>
              ))}
            </div>
          ) : null}
        </div>

        {/* GitHub OAuth */}
        <div className='rounded-md border border-hairline bg-surface-2 p-3.5 flex flex-col gap-3'>
          <div className='flex items-center justify-between gap-3'>
            <div className='flex items-center gap-2'>
              <span className='text-[13.5px] font-semibold'>GitHub</span>
              <Badge variant={saved?.github.enabled ? 'success' : 'outline'}>
                {saved?.github.enabled ? t('已启用') : t('已关闭')}
              </Badge>
            </div>
            <Switch
              id={`${prefix}-gh-enable`}
              checked={githubEnabled}
              disabled={locked}
              onCheckedChange={setGithubEnabled}
            />
          </div>

          <div className='grid grid-cols-1 gap-3 sm:grid-cols-2'>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-gh-id`}>Client ID</Label>
              <Input
                id={`${prefix}-gh-id`}
                value={githubClientId}
                disabled={locked}
                placeholder='Iv1.xxxx / Ov23li...'
                onChange={e => setGithubClientId(e.target.value)}
              />
            </div>
            <div className='space-y-1.5'>
              <Label htmlFor={`${prefix}-gh-secret`}>Client Secret</Label>
              <Input
                id={`${prefix}-gh-secret`}
                type='password'
                autoComplete='new-password'
                value={githubClientSecret}
                disabled={locked}
                placeholder={
                  saved?.github.clientSecretSet
                    ? t('已保存，留空则不修改')
                    : t('输入 GitHub Client Secret')
                }
                onChange={e => setGithubClientSecret(e.target.value)}
              />
            </div>
          </div>

          <div className='space-y-1.5'>
            <div className='flex items-center justify-between gap-2'>
              <Label htmlFor={`${prefix}-gh-users`}>{t('允许登录的 GitHub 用户名（每行一个，或点击右侧跳转绑定）')}</Label>
              <Button
                variant='outline'
                size='sm'
                disabled={
                  locked
                  || bindingProvider !== null
                  || !githubClientId.trim()
                  || (!githubClientSecret.trim() && !saved?.github.clientSecretSet)
                }
                onClick={() => void startBind('github')}
              >
                {bindingProvider === 'github' ? t('等待授权绑定…') : t('跳转绑定当前 GitHub 账号')}
              </Button>
            </div>
            <Textarea
              id={`${prefix}-gh-users`}
              rows={2}
              value={githubUsers}
              disabled={locked}
              placeholder='octocat'
              onChange={e => setGithubUsers(e.target.value)}
            />
          </div>

          {saved?.github.boundAccounts && saved.github.boundAccounts.length > 0 ? (
            <div className='flex flex-wrap items-center gap-2'>
              <span className='text-xs text-muted-foreground'>{t('已绑定：')}</span>
              {saved.github.boundAccounts.map(acc => (
                <Badge key={acc.id} variant='outline' className='flex items-center gap-1.5'>
                  <span>{acc.login || acc.email || acc.id}</span>
                  <button
                    type='button'
                    className='ml-1 cursor-pointer border-0 bg-transparent p-0 text-muted-foreground hover:text-destructive'
                    disabled={locked}
                    onClick={() =>
                      void save({ github: { unbindId: acc.id } }, t('已解除 GitHub 账号绑定'))
                    }
                  >
                    ×
                  </button>
                </Badge>
              ))}
            </div>
          ) : null}
        </div>

        {error ? (
          <Alert variant='destructive' className='whitespace-pre-wrap break-words'>
            {error}
          </Alert>
        ) : null}

        <div className='flex items-center justify-end'>
          <Button disabled={locked} onClick={() => void save(buildFullPatch())}>
            {busy ? t('保存中…') : t('保存')}
          </Button>
        </div>
      </CardContent>
    </Card>
  )
}
