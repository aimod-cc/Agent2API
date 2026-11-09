import * as React from 'react'
import {
  Alert, Badge, Button, Card, CardContent, CardHeader, CardTitle, Input, Label,
  Select, SelectContent, SelectItem, SelectTrigger, SelectValue, Switch, Textarea,
} from '@ui'
import type { CaptchaPatch, CaptchaProvider, CaptchaSetting } from '../../../ui-shared/panel-captcha'
import { errorMessage, shared, toast, TIPS } from './settings-model'
import { t } from '../i18n'

export function CaptchaSettingsPanel({ disabled = false }: { disabled?: boolean }) {
  const [saved, setSaved] = React.useState<CaptchaSetting | null>(null)
  const [provider, setProvider] = React.useState<CaptchaProvider>('altcha')
  const [siteKey, setSiteKey] = React.useState('')
  const [secretKey, setSecretKey] = React.useState('')
  const [hostnames, setHostnames] = React.useState('')
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const pending = React.useRef(false)
  const prefix = React.useId()
  const locked = disabled || busy || !saved

  const accept = React.useCallback((value: CaptchaSetting | null | undefined) => {
    if (!value || typeof value.captchaDesktop !== 'boolean' || typeof value.captchaWeb !== 'boolean') {
      throw new Error(t('未能读取机器人校验设置'))
    }
    setSaved(value)
    setProvider(value.captchaProvider || 'altcha')
    setSiteKey(value.turnstileSiteKey || '')
    setSecretKey('')
    setHostnames((value.turnstileHostnames || []).join('\n'))
  }, [])

  const load = React.useCallback(async () => {
    if (pending.current) return
    pending.current = true
    setBusy(true)
    setError('')
    try { accept(await shared().workbuddyDesktop?.getCaptchaSetting()) }
    catch (failure) { setError(errorMessage(failure)) }
    finally { pending.current = false; setBusy(false) }
  }, [accept])

  React.useEffect(() => { void load() }, [load])

  async function save(patch: CaptchaPatch, resetDraft: boolean) {
    if (pending.current || disabled) return
    pending.current = true
    setBusy(true)
    setError('')
    try {
      const result = await shared().workbuddyDesktop?.saveCaptchaSetting(patch)
      if (!result) throw new Error(t('未能读取机器人校验设置'))
      if (resetDraft) accept(result)
      else setSaved(result)
      toast(t('机器人校验设置已保存'))
    } catch (failure) { setError(errorMessage(failure)) }
    finally { pending.current = false; setBusy(false) }
  }

  const providerName = provider === 'turnstile' ? 'Cloudflare Turnstile' : t('ALTCHA（本地验证）')
  const incomplete = provider === 'turnstile' && (!siteKey.trim() || !hostnames.trim()
    || (!secretKey.trim() && !saved?.turnstileSecretSet))

  return <Card render={<section />} className='panel'>
    <CardHeader className='flex-row items-center justify-between gap-3'>
      <CardTitle>{t('机器人校验')}</CardTitle>
      <Button variant='ghost' size='sm' disabled={busy || disabled} onClick={() => void load()}>{t('刷新')}</Button>
    </CardHeader>
    <CardContent className='panel-body flex flex-col gap-4'>
      <div className='settings-switches'>
        {(['desktop', 'web'] as const).map(side => {
          const key = side === 'desktop' ? 'captchaDesktop' : 'captchaWeb'
          const current = saved && (side === 'desktop' ? saved.localPanel : !saved.localPanel)
          return <Label key={side} className='flex items-center justify-between gap-3' htmlFor={`${prefix}-${side}`}>
            <span>{side === 'desktop' ? t('桌面端登录 / 注册验证') : t('网页端登录 / 注册验证')}
              {current ? <Badge variant='outline' className='ml-2'>{t('当前正在使用')}</Badge> : null}</span>
            <Switch id={`${prefix}-${side}`} checked={saved?.[key] === true} disabled={locked}
              onCheckedChange={value => void save({ [key]: value }, false)} />
          </Label>
        })}
      </div>
      <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{TIPS.captchaSides}</p>
      <div className='space-y-2'>
        <Label htmlFor={`${prefix}-provider`}>{t('验证方式')}</Label>
        <Select value={provider} disabled={locked} onValueChange={value => setProvider(value as CaptchaProvider)}>
          <SelectTrigger id={`${prefix}-provider`}><SelectValue>{providerName}</SelectValue></SelectTrigger>
          <SelectContent>
            <SelectItem value='altcha'>{t('ALTCHA（本地验证）')}</SelectItem>
            <SelectItem value='turnstile'>Cloudflare Turnstile</SelectItem>
          </SelectContent>
        </Select>
        <p className='m-0 text-xs leading-relaxed text-muted-foreground'>
          {provider === 'altcha' ? t('浏览器在本地完成计算，不需要第三方密钥，也不依赖外部验证服务。')
            : t('由 Cloudflare 提供验证，需要浏览器与网关均能连接 Cloudflare。验证失败或服务不可用时不会自动放行。')}
        </p>
      </div>
      {provider === 'turnstile' ? <>
        <Alert>{t('请先在 Cloudflare 创建 Turnstile 组件，建议选择 Managed 模式，并添加面板实际使用的域名。')}</Alert>
        <div className='space-y-2'>
          <Label htmlFor={`${prefix}-site`}>{t('站点密钥（Site Key）')}</Label>
          <Input id={`${prefix}-site`} value={siteKey} disabled={locked} autoComplete='off' maxLength={512}
            onChange={event => setSiteKey(event.target.value)} />
        </div>
        <div className='space-y-2'>
          <Label htmlFor={`${prefix}-secret`}>{t('服务端密钥（Secret Key）')}</Label>
          <Input id={`${prefix}-secret`} type='password' value={secretKey} disabled={locked} autoComplete='new-password' maxLength={512}
            placeholder={saved?.turnstileSecretSet ? t('已保存，留空则不修改') : t('请输入服务端密钥')}
            onChange={event => setSecretKey(event.target.value)} />
          <p className='m-0 text-xs text-muted-foreground'>{t('服务端密钥加密保存，不会返回浏览器；切回 ALTCHA 后会保留，方便再次启用。')}</p>
        </div>
        <div className='space-y-2'>
          <Label htmlFor={`${prefix}-hosts`}>{t('允许的域名')}</Label>
          <Textarea id={`${prefix}-hosts`} value={hostnames} disabled={locked} rows={3} placeholder='panel.example.com'
            onChange={event => setHostnames(event.target.value)} />
          <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('每行一个域名，不填协议、端口或路径；与 Cloudflare 中的配置一致，服务端按完整域名匹配。手机 App 使用所连接网关的域名。')}</p>
        </div>
      </> : null}
      {error || saved?.error ? <Alert variant='destructive' className='whitespace-pre-wrap break-words'>{error || saved?.error}</Alert> : null}
      <div className='flex items-center justify-between gap-3'>
        <p className='m-0 text-xs text-muted-foreground'>{t('仅影响面板登录与注册，不影响 API Key 调用。保存后生效，已登录会话不会退出。')}</p>
        <Button disabled={locked || incomplete} onClick={() => void save({
          captchaProvider: provider,
          ...(provider === 'turnstile' ? {
            turnstileSiteKey: siteKey.trim(), turnstileHostnames: hostnames.split(/[\n,，]/).map(value => value.trim()).filter(Boolean),
            ...(secretKey.trim() ? { turnstileSecretKey: secretKey.trim() } : {}),
          } : {}),
        }, true)}>{busy ? t('保存中…') : t('保存')}</Button>
      </div>
    </CardContent>
  </Card>
}
