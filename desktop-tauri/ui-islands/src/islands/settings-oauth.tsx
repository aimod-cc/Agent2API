import * as React from 'react'
import {
  Alert,
  Badge,
  BadgeDot,
  Button,
  Card,
  CardAction,
  CardContent,
  CardFooter,
  CardHeader,
  CardTitle,
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemMedia,
  ItemTitle,
  Switch,
} from '@ui'
import { errorMessage, openExternal, shared, toast } from './settings-model'
import { GitHubMark, GoogleMark, RefreshIcon } from './settings-account-ui'
import { OauthProviderDialog, type OauthProvider } from './settings-oauth-dialog'
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

function OauthProviderTile({
  provider,
  saved,
  locked,
  onConfigure,
  onToggle,
}: {
  provider: OauthProvider
  saved: PanelOauthSettingsData | null
  locked: boolean
  onConfigure: () => void
  onToggle: (next: boolean) => void
}) {
  const isGoogle = provider === 'google'
  const label = isGoogle ? 'Google' : 'GitHub'
  const item = saved?.[provider]
  const bound = item?.boundAccounts || []
  const allowList = provider === 'google' ? (saved?.google.allowedEmails || []) : (saved?.github.allowedUsers || [])
  const configured = Boolean(item?.clientId?.trim()) && item?.clientSecretSet === true

  let summary = t('未配置')
  if (!saved) {
    summary = t('读取中…')
  } else if (bound.length > 0) {
    const first = isGoogle
      ? (bound[0].email || bound[0].name || bound[0].id)
      : (bound[0].login || bound[0].email || bound[0].id)
    summary = bound.length > 1
      ? t('已绑定 {first} 等 {count} 项', { first, count: bound.length })
      : t('已绑定 {name}', { name: first })
  } else if (allowList.length > 0) {
    summary = t('已配置 · 白名单 {count} 项', { count: allowList.length })
  } else if (configured) {
    summary = t('已配置凭据')
  }

  return (
    <Item variant='outline' className='flex-wrap bg-surface px-3.5 py-2.5 sm:flex-nowrap'>
      <ItemMedia>
        <div className='flex size-8 items-center justify-center rounded-sm border border-hairline bg-surface-2'>
          {isGoogle ? <GoogleMark size={16} /> : <GitHubMark size={16} />}
        </div>
      </ItemMedia>
      <ItemContent>
        <ItemTitle className='flex items-center gap-2 text-[13px] font-semibold'>
          <span>{label}</span>
          {item?.enabled ? <span className='size-1.5 rounded-full bg-success' aria-label={t('已启用')} /> : null}
        </ItemTitle>
        <ItemDescription className='truncate'>{summary}</ItemDescription>
      </ItemContent>
      <ItemActions>
        <Button variant='outline' size='sm' disabled={locked} onClick={onConfigure}>
          {t('配置')}
        </Button>
        <Switch
          checked={item?.enabled === true}
          disabled={locked}
          aria-label={isGoogle ? t('启用 Google 登录') : t('启用 GitHub 登录')}
          onCheckedChange={onToggle}
        />
      </ItemActions>
    </Item>
  )
}

export function OauthSettingsPanel() {
  const [saved, setSaved] = React.useState<PanelOauthSettingsData | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [bindingProvider, setBindingProvider] = React.useState<OauthProvider | null>(null)
  const [error, setError] = React.useState('')
  const [backendPort, setBackendPort] = React.useState<number>(3065)
  const [dialogState, setDialogState] = React.useState<{ provider: OauthProvider; forceEnable: boolean } | null>(null)

  const pollTimerRef = React.useRef<number | null>(null)

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

  const resolveBaseOrigin = React.useCallback((publicBaseUrl: string) => {
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
  }, [backendPort])

  async function save(
    patch: Record<string, unknown>,
    successMsg = t('第三方登录设置已保存'),
    throwOnError = false,
  ): Promise<boolean> {
    setBusy(true)
    setError('')
    try {
      const result = await shared().workbuddyDesktop?.saveOauthSettings?.(patch) as PanelOauthSettingsData | undefined
      accept(result)
      toast(successMsg, 'ok')
      return true
    } catch (failure) {
      const msg = errorMessage(failure)
      if (throwOnError) throw failure
      setError(msg)
      toast(msg, 'err')
      return false
    } finally {
      setBusy(false)
    }
  }

  async function startBind(provider: OauthProvider, origin: string, patch: Record<string, unknown>) {
    stopPolling()
    setBindingProvider(provider)
    setError('')
    try {
      const res = await shared().workbuddyDesktop?.startOauthBind?.({
        provider,
        origin,
        patch,
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
      setBindingProvider(null)
      throw failure
    }
  }

  async function handleToggle(provider: OauthProvider, next: boolean) {
    if (!saved || busy) return
    const item = saved[provider]
    const label = provider === 'google' ? 'Google' : 'GitHub'
    const configured = Boolean(item.clientId.trim()) && item.clientSecretSet === true
    if (next && !configured) {
      setDialogState({ provider, forceEnable: true })
      toast(t('请先填写 {provider} 应用凭据', { provider: label }), 'warn')
      return
    }
    await save(
      { [provider]: { enabled: next } },
      next ? t('已开启 {provider} 登录', { provider: label }) : t('已关闭 {provider} 登录', { provider: label }),
    )
  }

  const locked = busy || !saved
  const anyEnabled = saved !== null && (saved.google.enabled || saved.github.enabled)
  const customBaseUrl = saved?.publicBaseUrl?.trim() || ''

  return (
    <>
      <Card render={<section />} className='panel'>
        <CardHeader>
          <div className='flex flex-wrap items-center gap-2'>
            <CardTitle>{t('第三方登录 (OAuth)')}</CardTitle>
            {anyEnabled ? (
              <Badge variant='success'>
                <BadgeDot />
                {t('已启用')}
              </Badge>
            ) : (
              <Badge variant='outline'>{t('未启用')}</Badge>
            )}
          </div>
          <CardAction>
            <Button variant='outline' size='sm' disabled={busy} onClick={() => void load()}>
              <RefreshIcon />
              {t('刷新')}
            </Button>
          </CardAction>
        </CardHeader>

        <CardContent className='flex flex-col gap-3'>
          <div className='grid grid-cols-1 gap-3 lg:grid-cols-2'>
            <OauthProviderTile
              provider='google'
              saved={saved}
              locked={locked}
              onConfigure={() => setDialogState({ provider: 'google', forceEnable: false })}
              onToggle={next => void handleToggle('google', next)}
            />
            <OauthProviderTile
              provider='github'
              saved={saved}
              locked={locked}
              onConfigure={() => setDialogState({ provider: 'github', forceEnable: false })}
              onToggle={next => void handleToggle('github', next)}
            />
          </div>

          {error ? (
            <Alert variant='destructive' className='whitespace-pre-wrap break-words'>
              {error}
            </Alert>
          ) : null}
        </CardContent>

        {customBaseUrl ? (
          <CardFooter className='bg-surface-2/60'>
            <span>{t('外部访问地址：')}</span>
            <code className='font-mono text-[11.5px] text-foreground break-all'>{customBaseUrl}</code>
          </CardFooter>
        ) : null}
      </Card>

      <OauthProviderDialog
        provider={dialogState?.provider ?? null}
        open={dialogState !== null}
        forceEnable={dialogState?.forceEnable === true}
        saved={saved}
        busy={busy}
        bindingProvider={bindingProvider}
        resolveBaseOrigin={resolveBaseOrigin}
        onOpenChange={open => {
          if (!open) setDialogState(null)
        }}
        onSave={patch => save(patch, t('第三方登录设置已保存'), true)}
        onBind={startBind}
        onUnbind={async (provider, accountId) => {
          await save(
            { [provider]: { unbindId: accountId } },
            provider === 'google' ? t('已解除 Google 账号绑定') : t('已解除 GitHub 账号绑定'),
            true,
          )
        }}
      />
    </>
  )
}
