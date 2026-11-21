import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Spinner } from '@ui'
import { t } from '../ui-kit/src/i18n'
import type { CredentialFile, Method } from '../ui-islands/src/islands/add-account-dynamic-fields'
import { readFileAsDataUrl } from './file-read'
import { SecretTextarea } from './plugin-secret-textarea'

export type CredentialUpload = { name: string; base64?: string }

export function credentialInputMethod(method: Method, upload?: CredentialUpload): Method {
  const field = method.file?.field
  return upload?.base64 && field ? { ...method, prompts: method.prompts.filter(prompt => prompt.key !== field) } : method
}

export function credentialFilePayload(method: Method, upload: CredentialUpload | undefined, fields: Record<string, string>): Record<string, string> {
  if (upload?.base64 && method.file?.binary) return { authFileB64: upload.base64, authFileName: upload.name }
  const field = method.file?.field
  if (!upload || !field || Object.entries(fields).some(([key, value]) => key !== field && key !== 'name' && value.trim())) return {}
  return { authFileContent: fields[field], authFileName: upload.name }
}

export function PluginCredentialFile({ spec, value, upload, disabled, onChange, onUpload, onReading }: {
  spec: CredentialFile
  value: string
  upload?: CredentialUpload
  disabled?: boolean
  onChange(value: string): void
  onUpload(value?: CredentialUpload): void
  onReading?(value: boolean): void
}): React.ReactElement {
  const input = React.useRef<HTMLInputElement>(null)
  const [fileName, setFileName] = React.useState('')
  const [open, setOpen] = React.useState(false)
  const [draft, setDraft] = React.useState('')
  const [error, setError] = React.useState('')
  const [reading, setReading] = React.useState(false)
  const generation = React.useRef(0)
  const alive = React.useRef(true)
  const latest = React.useRef({ disabled, onChange, onUpload })
  latest.current = { disabled, onChange, onUpload }
  React.useEffect(() => { onReading?.(reading); return () => onReading?.(false) }, [reading, onReading])
  React.useEffect(() => { alive.current = true; return () => { alive.current = false; generation.current++ } }, [])
  React.useEffect(() => {
    if (!value && !upload) setFileName('')
    if (disabled) { generation.current++; setOpen(false); setDraft(''); setReading(false) }
  }, [disabled, value, upload])

  async function select(file: File): Promise<void> {
    const token = ++generation.current
    setError(''); setReading(true)
    const current = () => alive.current && token === generation.current && !latest.current.disabled
    try {
      if (!file.size || file.size > spec.maxBytes) throw new Error(t('请选择非空且不超过 {n} MiB 的凭据文件', { n: spec.maxBytes / 1048576 }))
      const header = new Uint8Array(await file.slice(0, 16).arrayBuffer())
      const sqlite = String.fromCharCode(...header) === 'SQLite format 3\0'
      if (!current()) return
      if (sqlite) {
        if (!spec.binary) throw new Error(t('此登录方式只接受文本凭据文件'))
        const data = await readFileAsDataUrl(file, t('文件读取失败'))
        if (!current()) return
        latest.current.onChange('')
        latest.current.onUpload({ name: file.name, base64: data.slice(data.indexOf(',') + 1) })
      } else {
        if (file.size > 262144) throw new Error(t('文本凭据文件不能超过 256 KiB'))
        const text = new TextDecoder('utf-8', { fatal: true }).decode(await file.arrayBuffer()).replace(/^\uFEFF/, '')
        if (!current()) return
        if (!text.trim() || text.includes('\0')) throw new Error(t('请选择有效的文本凭据文件'))
        latest.current.onUpload({ name: file.name })
        latest.current.onChange(text)
      }
      if (current()) setFileName(file.name)
    } catch (cause) { if (current()) setError(cause instanceof Error ? cause.message : t('文件读取失败')) }
    finally { if (current()) setReading(false) }
  }

  function close(): void { setOpen(false); setDraft('') }
  const textSize = new TextEncoder().encode(draft).length
  const validDraft = Boolean(draft.trim()) && !draft.includes('\0') && textSize <= Math.min(spec.maxBytes, 262144)

  return <div className='plugin-login-field'>
    <Input ref={input} type='file' accept={spec.accept} className='hidden' disabled={disabled || reading}
      onChange={event => {
        const file = event.currentTarget.files?.[0]
        event.currentTarget.value = ''
        if (file) void select(file)
      }} />
    <div className='flex flex-wrap items-center gap-2'>
      <Button type='button' variant='outline' disabled={disabled || reading} onClick={() => input.current?.click()}>
        {reading ? <Spinner /> : null}{t('选择凭据文件')}
      </Button>
      <Button type='button' variant='link' disabled={disabled || reading} onClick={() => { setDraft(value); setOpen(true); setError('') }}>
        {t('手动粘贴')}
      </Button>
      {value || upload ? <span className='min-w-0 break-all text-xs text-muted-foreground'>{upload?.name || fileName || t('已填写凭据')}</span> : null}
    </div>
    {error ? <Alert variant='destructive'>{error}</Alert> : null}
    <Dialog open={open && !disabled} onOpenChange={next => { if (!next) close() }}>
      <DialogContent overlayForceRender className='w-[min(620px,calc(100vw-32px))]'>
        <DialogHeader><DialogTitle>{t('粘贴凭据内容')}</DialogTitle></DialogHeader>
        <DialogBody><SecretTextarea label={t('凭据内容')} value={draft} rows={9} maxLength={262144}
          autoComplete='off' autoCapitalize='none' spellCheck={false} onChange={event => setDraft(event.currentTarget.value)} />
          {textSize > Math.min(spec.maxBytes, 262144) ? <Alert variant='destructive'>{t('文本凭据文件不能超过 256 KiB')}</Alert> : null}
        </DialogBody>
        <DialogFooter>
          <Button type='button' variant='outline' onClick={close}>{t('取消')}</Button>
          <Button type='button' disabled={!validDraft} onClick={() => {
            generation.current++; onUpload(undefined); onChange(draft); setFileName(''); close()
          }}>{t('使用此凭据')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
}
