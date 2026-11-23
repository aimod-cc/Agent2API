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
  Dialog,
  DialogBody,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Field,
  FieldDescription,
  FieldLabel,
  Input,
  Item,
  ItemActions,
  ItemContent,
  ItemDescription,
  ItemGroup,
  ItemMedia,
  ItemTitle,
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
  Switch,
  Textarea,
} from '@ui'
import type { CaptchaPatch, CaptchaProvider, CaptchaSetting } from '../../../ui-shared/panel-captcha'
import { errorMessage, shared, toast } from './settings-model'
import { RefreshIcon, SecretInput } from './settings-account-ui'
import { t } from '../i18n'

function iconHtml(name: string, size: number): string {
  return shared().wbIcons?.icon?.(name, size) || ''
}

function TurnstileDialog({
  open,
  busy,
  initialSiteKey,
  initialSecretSet,
  initialHostnames,
  onOpenChange,
  onSave,
}: {
  open: boolean
  busy: boolean
  initialSiteKey: string
  initialSecretSet: boolean
  initialHostnames: string[]
  onOpenChange: (next: boolean) => void
  onSave: (patch: CaptchaPatch) => Promise<boolean>
}) {
  const prefix = React.useId()
  const [siteKey, setSiteKey] = React.useState(initialSiteKey)
  const [secretKey, setSecretKey] = React.useState('')
  const [hostnames, setHostnames] = React.useState(initialHostnames.join('\n'))
  const [dialogError, setDialogError] = React.useState('')

  React.useEffect(() => {
    if (open) {
      setSiteKey(initialSiteKey)
      setSecretKey('')
      setHostnames(initialHostnames.join('\n'))
      setDialogError('')
    }
  }, [open, initialSiteKey, initialHostnames])

  const incomplete = !siteKey.trim() || !hostnames.trim() || (!secretKey.trim() && !initialSecretSet)

  async function submit(event: React.FormEvent) {
    event.preventDefault()
    if (busy || incomplete) return
    setDialogError('')
    try {
      const ok = await onSave({
        captchaProvider: 'turnstile',
        turnstileSiteKey: siteKey.trim(),
        turnstileHostnames: hostnames.split(/[\n,，]/).map(value => value.trim()).filter(Boolean),
        ...(secretKey.trim() ? { turnstileSecretKey: secretKey.trim() } : {}),
      })
      if (ok) onOpenChange(false)
    } catch (failure) {
      setDialogError(errorMessage(failure))
    }
  }

  return (
    <Dialog open={open} onOpenChange={next => { if (!busy) onOpenChange(next) }}>
      <DialogContent className='w-[min(520px,calc(100vw-48px))]'>
        <DialogHeader>
          <div className='min-w-0 flex-1'>
            <DialogTitle>{t('配置 Cloudflare Turnstile')}</DialogTitle>
            <DialogDescription className='mt-0.5 text-[12px] text-subtle'>
              {t('由 Cloudflare 提供验证，需要浏览器与网关均能连接 Cloudflare。验证失败或服务不可用时不会自动放行。')}
            </DialogDescription>
          </div>
        </DialogHeader>
        <form onSubmit={event => { void submit(event) }}>
          <DialogBody className='gap-3.5'>
            <Alert>{t('请先在 Cloudflare 创建 Turnstile 组件，建议选择 Managed 模式，并添加面板实际使用的域名。')}</Alert>

            <div className='grid grid-cols-1 gap-3.5 sm:grid-cols-2'>
              <Field>
                <FieldLabel htmlFor={`${prefix}-site`} className='text-[12.5px] font-medium text-foreground'>
                  {t('站点密钥（Site Key）')}
                </FieldLabel>
                <Input
                  id={`${prefix}-site`}
                  value={siteKey}
                  disabled={busy}
                  autoComplete='off'
                  maxLength={512}
                  placeholder='0x4AAAAAAA...'
                  onChange={event => setSiteKey(event.target.value)}
                />
              </Field>

              <Field>
                <FieldLabel htmlFor={`${prefix}-secret`} className='text-[12.5px] font-medium text-foreground'>
                  {t('服务端密钥（Secret Key）')}
                </FieldLabel>
                <SecretInput
                  id={`${prefix}-secret`}
                  value={secretKey}
                  disabled={busy}
                  placeholder={initialSecretSet ? t('已保存，留空则不修改') : t('请输入服务端密钥')}
                  onChange={setSecretKey}
                />
              </Field>
            </div>

            <Field>
              <FieldLabel htmlFor={`${prefix}-hosts`} className='text-[12.5px] font-medium text-foreground'>
                {t('允许的域名')}
              </FieldLabel>
              <Textarea
                id={`${prefix}-hosts`}
                rows={3}
                value={hostnames}
                disabled={busy}
                placeholder='panel.example.com'
                onChange={event => setHostnames(event.target.value)}
              />
              <FieldDescription>
                {t('每行一个域名，不填协议、端口或路径；与 Cloudflare 中的配置一致，服务端按完整域名匹配。手机 App 使用所连接网关的域名。')}
              </FieldDescription>
            </Field>

            {dialogError ? (
              <Alert variant='destructive' className='whitespace-pre-wrap break-words'>
                {dialogError}
              </Alert>
            ) : null}
          </DialogBody>

          <DialogFooter className='justify-between gap-3'>
            <span className='mr-auto text-[11.5px] text-muted-foreground'>
              {t('切回 ALTCHA 后密钥仍会保留，方便再次启用。')}
            </span>
            <Button type='button' variant='outline' disabled={busy} onClick={() => onOpenChange(false)}>
              {t('取消')}
            </Button>
            <Button type='submit' disabled={busy || incomplete}>
              {busy ? t('保存中…') : t('保存并启用')}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

export function CaptchaSettingsPanel({ disabled = false }: { disabled?: boolean }) {
  const [saved, setSaved] = React.useState<CaptchaSetting | null>(null)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const [turnstileOpen, setTurnstileOpen] = React.useState(false)
  const pending = React.useRef(false)
  const prefix = React.useId()
  const locked = disabled || busy || !saved

  const accept = React.useCallback((value: CaptchaSetting | null | undefined) => {
    if (!value || typeof value.captchaDesktop !== 'boolean' || typeof value.captchaWeb !== 'boolean') {
      throw new Error(t('未能读取机器人校验设置'))
    }
    setSaved(value)
  }, [])

  const load = React.useCallback(async () => {
    if (pending.current) return
    pending.current = true
    setBusy(true)
    setError('')
    try {
      accept(await shared().workbuddyDesktop?.getCaptchaSetting())
    } catch (failure) {
      setError(errorMessage(failure))
    } finally {
      pending.current = false
      setBusy(false)
    }
  }, [accept])

  React.useEffect(() => {
    void load()
  }, [load])

  async function save(patch: CaptchaPatch, throwOnError = false): Promise<boolean> {
    if (pending.current || disabled) return false
    pending.current = true
    setBusy(true)
    setError('')
    try {
      const result = await shared().workbuddyDesktop?.saveCaptchaSetting(patch)
      if (!result) throw new Error(t('未能读取机器人校验设置'))
      accept(result)
      toast(t('机器人校验设置已保存'))
      return true
    } catch (failure) {
      const msg = errorMessage(failure)
      if (throwOnError) throw failure
      setError(msg)
      return false
    } finally {
      pending.current = false
      setBusy(false)
    }
  }

  const provider: CaptchaProvider = saved?.captchaProvider || 'altcha'
  const providerName = provider === 'turnstile' ? 'Cloudflare Turnstile' : t('ALTCHA（本地验证）')
  const hostCount = saved?.turnstileHostnames?.length ?? 0
  const turnstileConfigured =
    Boolean(saved?.turnstileSiteKey?.trim()) && hostCount > 0 && saved?.turnstileSecretSet === true

  async function handleSelectProvider(next: CaptchaProvider) {
    if (locked || next === provider) return
    if (next === 'altcha') {
      await save({ captchaProvider: 'altcha' })
      return
    }
    if (!turnstileConfigured) {
      setTurnstileOpen(true)
      return
    }
    try {
      await save({ captchaProvider: 'turnstile' }, true)
    } catch {
      setTurnstileOpen(true)
    }
  }

  return (
    <>
      <Card render={<section />} className='panel'>
        <CardHeader>
          <div className='min-w-0 flex-1'>
            <CardTitle>{t('机器人校验')}</CardTitle>
          </div>
          <CardAction>
            <Button variant='outline' size='sm' disabled={busy || disabled} onClick={() => void load()}>
              <RefreshIcon />
              {t('刷新')}
            </Button>
          </CardAction>
        </CardHeader>

        <CardContent className='flex flex-col gap-3.5'>
          <ItemGroup className='grid grid-cols-1 divide-y divide-hairline overflow-hidden rounded-md border border-border bg-surface lg:grid-cols-2 lg:divide-x lg:divide-y-0'>
            {(['desktop', 'web'] as const).map(side => {
              const key = side === 'desktop' ? 'captchaDesktop' : 'captchaWeb'
              const current = saved && (side === 'desktop' ? saved.localPanel : !saved.localPanel)
              const iconName = side === 'desktop' ? 'display' : 'globe'
              const inputId = `${prefix}-${side}`
              return (
                <Item key={side} className='rounded-none px-3.5 py-2.5 hover:bg-surface-2/60'>
                  <ItemMedia>
                    <div
                      className='flex size-8 items-center justify-center rounded-sm border border-hairline bg-surface-2 text-subtle [&_svg]:block'
                      aria-hidden='true'
                      dangerouslySetInnerHTML={{ __html: iconHtml(iconName, 16) }}
                    />
                  </ItemMedia>
                  <ItemContent>
                    <label htmlFor={inputId} className='block cursor-pointer'>
                      <ItemTitle className='flex flex-wrap items-center gap-2'>
                        <span>{side === 'desktop' ? t('桌面端登录 / 注册') : t('网页端登录 / 注册')}</span>
                        {current ? <Badge variant='brand' shape='tag'>{t('当前使用')}</Badge> : null}
                      </ItemTitle>
                      <ItemDescription>
                        {side === 'desktop' ? t('本机桌面托管面板') : t('Docker / 服务端面板，建议开启')}
                      </ItemDescription>
                    </label>
                  </ItemContent>
                  <ItemActions>
                    <Switch
                      id={inputId}
                      checked={saved?.[key] === true}
                      disabled={locked}
                      onCheckedChange={value => void save({ [key]: value })}
                    />
                  </ItemActions>
                </Item>
              )
            })}
          </ItemGroup>

          <Field>
            <FieldLabel htmlFor={`${prefix}-provider`} className='text-[12.5px] font-medium text-foreground'>
              {t('验证方式')}
            </FieldLabel>
            <div className='flex flex-wrap items-center gap-2.5'>
              <Select
                value={provider}
                disabled={locked}
                onValueChange={value => {
                  if (value) void handleSelectProvider(value as CaptchaProvider)
                }}
              >
                <SelectTrigger id={`${prefix}-provider`} className='w-full sm:w-[240px]'>
                  <SelectValue>{providerName}</SelectValue>
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value='altcha'>{t('ALTCHA（本地验证）')}</SelectItem>
                  <SelectItem value='turnstile'>Cloudflare Turnstile</SelectItem>
                </SelectContent>
              </Select>

              {provider === 'turnstile' ? (
                <div className='flex flex-wrap items-center gap-2'>
                  <Button variant='outline' size='sm' disabled={locked} onClick={() => setTurnstileOpen(true)}>
                    {t('配置密钥与域名')}
                  </Button>
                  {turnstileConfigured ? (
                    <Badge variant='success' shape='tag'>
                      <BadgeDot />
                      {t('已配置 {n} 个域名', { n: hostCount })}
                    </Badge>
                  ) : (
                    <Badge variant='warning' shape='tag'>
                      {t('待配置密钥')}
                    </Badge>
                  )}
                </div>
              ) : null}
            </div>
            <FieldDescription>
              {provider === 'altcha'
                ? t('浏览器在本地完成计算，不需要第三方密钥，也不依赖外部验证服务。')
                : t('由 Cloudflare 提供验证，需要浏览器与网关均能连接 Cloudflare。验证失败或服务不可用时不会自动放行。')}
            </FieldDescription>
          </Field>

          {error || saved?.error ? (
            <Alert variant='destructive' className='whitespace-pre-wrap break-words'>
              {error || saved?.error}
            </Alert>
          ) : null}
        </CardContent>

        <CardFooter className='bg-surface-2/60'>
          {t('仅影响面板登录与注册，不影响 API Key 调用。保存后生效，已登录会话不会退出。')}
        </CardFooter>
      </Card>

      <TurnstileDialog
        open={turnstileOpen}
        busy={busy}
        initialSiteKey={saved?.turnstileSiteKey || ''}
        initialSecretSet={saved?.turnstileSecretSet === true}
        initialHostnames={saved?.turnstileHostnames || []}
        onOpenChange={setTurnstileOpen}
        onSave={patch => save(patch, true)}
      />
    </>
  )
}
