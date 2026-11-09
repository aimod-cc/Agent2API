import * as React from 'react'
import { Alert, Button, Card, CardContent, CardHeader, CardTitle, Input, Label } from '@ui'
import { BrandLogo } from '../../../ui-shared/brand-logo'
import { DEFAULT_BRANDING, MAX_LOGO_BYTES } from '../../../ui-shared/branding'
import { desktopBranding, saveBranding } from '../branding'
import { t } from '../i18n'
import { errorMessage, toast } from './settings-model'

export function BrandingSettingsPanel({ disabled = false }: { disabled?: boolean }) {
  const branding = desktopBranding.useValue()
  const [name, setName] = React.useState(branding.displayName)
  const [logo, setLogo] = React.useState<string | null | undefined>(undefined)
  const [ready, setReady] = React.useState(false)
  const [reload, setReload] = React.useState(0)
  const [dirty, setDirty] = React.useState(false)
  const [busy, setBusy] = React.useState(false)
  const [error, setError] = React.useState('')
  const pending = React.useRef(false)
  const file = React.useRef<HTMLInputElement>(null)
  const id = React.useId()

  React.useEffect(() => {
    let alive = true
    setError('')
    void desktopBranding.refresh().then(value => {
      if (alive) { setName(value.displayName); setReady(true) }
    }).catch(failure => { if (alive) setError(errorMessage(failure)) })
    return () => { alive = false }
  }, [reload])

  React.useEffect(() => {
    if (!dirty) setName(branding.displayName)
  }, [branding.displayName, dirty])

  async function pick(selected?: File) {
    if (!selected || pending.current || disabled) return
    setError('')
    if (!['image/png', 'image/jpeg', 'image/webp'].includes(selected.type)) {
      setError(t('Logo 仅支持 PNG、JPG、WebP 图片'))
      return
    }
    if (!selected.size || selected.size > MAX_LOGO_BYTES) {
      setError(t('Logo 不能超过 2 MiB'))
      return
    }
    pending.current = true
    setBusy(true)
    try {
      const data = await new Promise<string>((resolve, reject) => {
        const reader = new FileReader()
        reader.onload = () => typeof reader.result === 'string' ? resolve(reader.result) : reject(new Error(t('无法读取图片，请重新选择')))
        reader.onerror = () => reject(new Error(t('无法读取图片，请重新选择')))
        reader.readAsDataURL(selected)
      })
      setLogo(data)
      setDirty(true)
    } catch (failure) { setError(errorMessage(failure)) }
    finally { pending.current = false; setBusy(false) }
  }

  async function save() {
    if (pending.current || disabled || !ready) return
    if (Array.from(name.trim()).length > 64) { setError(t('显示名称最多 64 个字符')); return }
    pending.current = true
    setBusy(true)
    setError('')
    try {
      const value = await saveBranding({ displayName: name.trim(), ...(logo !== undefined ? { logo } : {}) })
      setName(value.displayName)
      setLogo(undefined)
      setDirty(false)
      setReady(true)
      toast(t('品牌设置已保存'))
    } catch (failure) { setError(errorMessage(failure)) }
    finally { pending.current = false; setBusy(false) }
  }

  const preview = { displayName: name.trim() || DEFAULT_BRANDING.displayName, logoUrl: logo === undefined ? branding.logoUrl : logo }
  const locked = disabled || busy || !ready
  return <Card render={<section />} className='panel'>
    <CardHeader><CardTitle>{t('品牌设置')}</CardTitle></CardHeader>
    <CardContent className='panel-body flex flex-col gap-4'>
      <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('自定义网站和应用界面中显示的名称与 Logo。')}</p>
      <div className='flex min-w-0 items-center gap-4 rounded-lg border border-hairline bg-surface-inset p-4'>
        <BrandLogo branding={preview} size={48} />
        <div className='min-w-0'><div className='truncate text-base font-semibold'>{preview.displayName}</div>
          <div className='text-xs text-muted-foreground'>{t('品牌预览')}</div></div>
      </div>
      <div className='space-y-2'>
        <Label htmlFor={`${id}-name`}>{t('显示名称')}</Label>
        <Input id={`${id}-name`} value={name} disabled={locked} autoComplete='off' placeholder='Agent2API'
          onChange={event => { setName(event.target.value); setDirty(true) }} />
        <p className='m-0 text-xs text-muted-foreground'>{t('最多 64 个字符，留空恢复为 Agent2API。')}</p>
      </div>
      <div className='space-y-2'>
        <Label htmlFor={`${id}-logo`}>{t('品牌 Logo')}</Label>
        <input id={`${id}-logo`} ref={file} type='file' className='sr-only' accept='image/png,image/jpeg,image/webp' disabled={locked}
          onChange={event => { const selected = event.target.files?.[0]; event.target.value = ''; void pick(selected) }} />
        <div className='flex flex-wrap gap-2'>
          <Button variant='outline' disabled={locked} onClick={() => file.current?.click()}>{t('上传 Logo')}</Button>
          <Button variant='ghost' disabled={locked || !preview.logoUrl} onClick={() => { setLogo(null); setDirty(true) }}>{t('恢复默认 Logo')}</Button>
        </div>
        <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('支持 PNG、JPG、WebP，最大 2 MiB、4096 × 4096 像素；自动缩小至 512 像素以内，建议使用正方形图片。')}</p>
      </div>
      <p className='m-0 text-xs leading-relaxed text-muted-foreground'>{t('保存在当前网关，登录页也会公开显示。不会更改安装包、快捷方式、系统托盘图标或软件版本信息。')}</p>
      {error ? <Alert variant='destructive' className='break-words'>{error}</Alert> : null}
      {!ready && error ? <Button variant='outline' disabled={disabled} onClick={() => setReload(value => value + 1)}>{t('重试')}</Button> : null}
      <div className='flex items-center justify-end gap-2'>
        <Button variant='ghost' disabled={locked} onClick={() => { setName(DEFAULT_BRANDING.displayName); setLogo(null); setDirty(true); setError('') }}>{t('恢复默认品牌')}</Button>
        <Button disabled={locked} onClick={() => void save()}>{busy ? t('保存中…') : t('保存')}</Button>
      </div>
    </CardContent>
  </Card>
}
