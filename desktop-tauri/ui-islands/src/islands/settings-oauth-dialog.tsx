import * as React from 'react'
import {
  Alert,
  Badge,
  BadgeDot,
  Button,
  Dialog,
  DialogBody,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Field,
  FieldDescription,
  FieldLabel,
  Input,
  InputGroup,
  InputGroupAddon,
  InputGroupButton,
  InputGroupInput,
  Label,
  Switch,
  Textarea,
} from '@ui'
import { errorMessage, toast } from './settings-model'
import {
  CheckIcon,
  CopyIcon,
  ExternalLinkIcon,
  GitHubMark,
  GoogleMark,
  SecretInput,
} from './settings-account-ui'
import type { BoundOauthAccount, PanelOauthSettingsData } from './settings-oauth'
import { t } from '../i18n'

export type OauthProvider = 'google' | 'github'

function splitAllowList(raw: string): string[] {
  return raw
    .split(/[\n,，;；]/)
    .map(item => item.trim())
    .filter(Boolean)
}

export function OauthProviderDialog({
  provider,
  open,
  forceEnable = false,
  saved,
  busy,
  bindingProvider,
  resolveBaseOrigin,
  onOpenChange,
  onSave,
  onBind,
  onUnbind,
}: {
  provider: OauthProvider | null
  open: boolean
  forceEnable?: boolean
  saved: PanelOauthSettingsData | null
  busy: boolean
  bindingProvider: OauthProvider | null
  resolveBaseOrigin: (publicBaseUrl: string) => string
  onOpenChange: (next: boolean) => void
  onSave: (patch: Record<string, unknown>) => Promise<boolean>
  onBind: (provider: OauthProvider, origin: string, patch: Record<string, unknown>) => Promise<void>
  onUnbind: (provider: OauthProvider, accountId: string) => Promise<void>
}) {
  const prefix = React.useId()
  const [baseUrl, setBaseUrl] = React.useState('')
  const [enabled, setEnabled] = React.useState(false)
  const [clientId, setClientId] = React.useState('')
  const [clientSecret, setClientSecret] = React.useState('')
  const [allowList, setAllowList] = React.useState('')
  const [copied, setCopied] = React.useState(false)
  const [dialogError, setDialogError] = React.useState('')

  React.useEffect(() => {
    if (!open || !provider || !saved) return
    const item = saved[provider]
    setBaseUrl(saved.publicBaseUrl || '')
    setEnabled(forceEnable ? true : item.enabled === true)
    setClientId(item.clientId || '')
    setClientSecret('')
    const entries = provider === 'google' ? saved.google.allowedEmails : saved.github.allowedUsers
    setAllowList((entries || []).join('\n'))
    setCopied(false)
    setDialogError('')
  }, [open, provider, saved, forceEnable])

  if (!provider || !saved) return null
  const activeProvider = provider
  const isGoogle = activeProvider === 'google'
  const providerData = saved[activeProvider]
  const boundAccounts: BoundOauthAccount[] = providerData.boundAccounts || []
  const hasSecret = Boolean(clientSecret.trim()) || providerData.clientSecretSet
  const canBind = !busy && bindingProvider === null && Boolean(clientId.trim()) && hasSecret

  const baseOrigin = resolveBaseOrigin(baseUrl)
  const callbackUrl = `${baseOrigin}/api/panel/oauth/callback`

  function buildPatch(): Record<string, unknown> {
    const allowField = isGoogle ? 'allowedEmails' : 'allowedUsers'
    return {
      publicBaseUrl: baseUrl.trim(),
      [activeProvider]: {
        enabled,
        clientId: clientId.trim(),
        [allowField]: splitAllowList(allowList),
        ...(clientSecret.trim() ? { clientSecret: clientSecret.trim() } : {}),
      },
    }
  }

  async function copyCallback() {
    try {
      await navigator.clipboard.writeText(callbackUrl)
      toast(t('回调地址已复制'), 'ok')
    } catch {
      toast(callbackUrl, 'warn')
    }
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1800)
  }

  async function handleBind() {
    if (!canBind) return
    setDialogError('')
    try {
      await onBind(activeProvider, baseOrigin, buildPatch())
    } catch (failure) {
      setDialogError(errorMessage(failure))
    }
  }

  async function handleUnbind(id: string) {
    if (busy) return
    setDialogError('')
    try {
      await onUnbind(activeProvider, id)
    } catch (failure) {
      setDialogError(errorMessage(failure))
    }
  }

  async function handleSubmit(event: React.FormEvent) {
    event.preventDefault()
    if (busy) return
    setDialogError('')
    try {
      const ok = await onSave(buildPatch())
      if (ok) onOpenChange(false)
    } catch (failure) {
      setDialogError(errorMessage(failure))
    }
  }

  return (
    <Dialog open={open} onOpenChange={next => { if (!busy) onOpenChange(next) }}>
      <DialogContent className='w-[min(580px,calc(100vw-48px))]'>
        <DialogHeader>
          <div className='flex min-w-0 flex-1 items-center gap-2.5'>
            <div className='flex size-7 shrink-0 items-center justify-center rounded-sm border border-hairline bg-surface-2'>
              {isGoogle ? <GoogleMark size={15} /> : <GitHubMark size={15} />}
            </div>
            <DialogTitle>{isGoogle ? t('配置 Google 登录') : t('配置 GitHub 登录')}</DialogTitle>
          </div>
        </DialogHeader>

        <form onSubmit={event => { void handleSubmit(event) }}>
          <DialogBody className='gap-4'>
            <div className='flex flex-col gap-3 rounded-md border border-border bg-surface-2/60 p-3'>
              <Field>
                <FieldLabel htmlFor={`${prefix}-base`} className='text-[12px] font-medium text-foreground'>
                  {t('面板外部访问地址')}
                </FieldLabel>
                <Input
                  id={`${prefix}-base`}
                  value={baseUrl}
                  disabled={busy}
                  placeholder='https://panel.example.com'
                  onChange={event => setBaseUrl(event.target.value)}
                />
                <FieldDescription>
                  {t('反代或自定义域名时填写，Google 与 GitHub 共用；留空使用当前地址。')}
                </FieldDescription>
              </Field>

              <Field>
                <FieldLabel htmlFor={`${prefix}-callback`} className='text-[12px] font-medium text-foreground'>
                  {isGoogle ? t('回调地址（登记到 Google 应用）') : t('回调地址（登记到 GitHub 应用）')}
                </FieldLabel>
                <InputGroup className='bg-surface'>
                  <InputGroupInput
                    id={`${prefix}-callback`}
                    readOnly
                    value={callbackUrl}
                    className='font-mono text-[12px] text-subtle select-all'
                  />
                  <InputGroupAddon align='inline-end'>
                    <InputGroupButton variant='ghost' size='xs' onClick={() => void copyCallback()}>
                      {copied ? <CheckIcon /> : <CopyIcon />}
                      <span>{copied ? t('已复制') : t('复制')}</span>
                    </InputGroupButton>
                  </InputGroupAddon>
                </InputGroup>
              </Field>
            </div>

            <div className='grid grid-cols-1 gap-3.5 sm:grid-cols-2'>
              <Field>
                <FieldLabel htmlFor={`${prefix}-id`} className='text-[12.5px] font-medium text-foreground'>
                  Client ID
                </FieldLabel>
                <Input
                  id={`${prefix}-id`}
                  value={clientId}
                  disabled={busy}
                  placeholder={isGoogle ? 'xxxx.apps.googleusercontent.com' : 'Iv1.xxxx / Ov23li...'}
                  onChange={event => setClientId(event.target.value)}
                />
              </Field>

              <Field>
                <FieldLabel htmlFor={`${prefix}-secret`} className='text-[12.5px] font-medium text-foreground'>
                  Client Secret
                </FieldLabel>
                <SecretInput
                  id={`${prefix}-secret`}
                  value={clientSecret}
                  disabled={busy}
                  placeholder={
                    providerData.clientSecretSet
                      ? t('已保存，留空则不修改')
                      : isGoogle
                        ? t('输入 Google Client Secret')
                        : t('输入 GitHub Client Secret')
                  }
                  onChange={setClientSecret}
                />
              </Field>
            </div>

            <div className='flex flex-col gap-2'>
              <div className='flex flex-wrap items-center justify-between gap-2'>
                <Label htmlFor={`${prefix}-allow`} className='text-[12.5px] font-medium text-foreground'>
                  {isGoogle ? t('允许登录的 Google 邮箱') : t('允许登录的 GitHub 用户名')}
                </Label>
                <Button type='button' variant='outline' size='sm' disabled={!canBind} onClick={() => void handleBind()}>
                  <ExternalLinkIcon />
                  {bindingProvider === activeProvider
                    ? t('等待授权绑定…')
                    : isGoogle
                      ? t('绑定当前 Google 账号')
                      : t('绑定当前 GitHub 账号')}
                </Button>
              </div>

              {boundAccounts.length > 0 ? (
                <div className='flex flex-wrap items-center gap-1.5'>
                  <span className='text-[11.5px] text-muted-foreground'>{t('已绑定：')}</span>
                  {boundAccounts.map(acc => {
                    const display = isGoogle ? (acc.email || acc.name || acc.id) : (acc.login || acc.email || acc.id)
                    return (
                      <Badge key={acc.id} variant='outline' className='gap-1.5 pr-1.5'>
                        <BadgeDot className='text-success' />
                        <span>{display}</span>
                        <button
                          type='button'
                          aria-label={t('解除绑定 {name}', { name: display })}
                          className='ml-0.5 inline-flex size-4 cursor-pointer items-center justify-center rounded-full border-0 bg-transparent p-0 text-muted-foreground transition-colors hover:bg-destructive-soft hover:text-destructive'
                          disabled={busy}
                          onClick={() => void handleUnbind(acc.id)}
                        >
                          ×
                        </button>
                      </Badge>
                    )
                  })}
                </div>
              ) : null}

              <Textarea
                id={`${prefix}-allow`}
                rows={2}
                value={allowList}
                disabled={busy}
                placeholder={isGoogle ? 'yourname@gmail.com' : 'octocat'}
                onChange={event => setAllowList(event.target.value)}
              />
              <p className='m-0 text-[11.5px] leading-relaxed text-subtle'>
                {isGoogle
                  ? t('每行填写一个；留空且未绑定账号时将拒绝通过 Google 登录。')
                  : t('每行填写一个；留空且未绑定账号时将拒绝通过 GitHub 登录。')}
              </p>
            </div>

            {dialogError ? (
              <Alert variant='destructive' className='whitespace-pre-wrap break-words'>
                {dialogError}
              </Alert>
            ) : null}
          </DialogBody>

          <DialogFooter className='justify-between gap-3'>
            <label htmlFor={`${prefix}-enable`} className='flex cursor-pointer items-center gap-2 text-[12.5px] text-foreground select-none'>
              <Switch
                id={`${prefix}-enable`}
                size='sm'
                checked={enabled}
                disabled={busy}
                onCheckedChange={setEnabled}
              />
              <span>{isGoogle ? t('启用 Google 登录') : t('启用 GitHub 登录')}</span>
            </label>
            <div className='flex items-center gap-2'>
              <Button type='button' variant='outline' disabled={busy} onClick={() => onOpenChange(false)}>
                {t('取消')}
              </Button>
              <Button type='submit' disabled={busy}>
                {busy ? t('保存中…') : t('保存')}
              </Button>
            </div>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
