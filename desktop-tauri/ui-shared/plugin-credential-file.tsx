import * as React from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Spinner } from '@ui'
import { t } from '../ui-kit/src/i18n'
import type { CredentialFile, Method } from '../ui-islands/src/islands/add-account-dynamic-fields'
import { readFileAsDataUrl } from './file-read'
import { SecretTextarea } from './plugin-secret-textarea'

/**
 * 一份已读好的凭据文件：二进制给 `base64`，文本由所在输入框的字段值携带。
 * 多选时第一份照常填进表单，其余各份挂在它的 `batch` 上（文本带 `text`），提交时一起发。
 */
export type CredentialUpload = { name: string; base64?: string; text?: string; batch?: CredentialUpload[] }

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

  /** 读一份凭据文件；文件不合格直接抛错，文案是给用户看的 */
  async function read(file: File): Promise<CredentialUpload> {
    if (!file.size || file.size > spec.maxBytes) throw new Error(t('请选择非空且不超过 {n} MiB 的凭据文件', { n: spec.maxBytes / 1048576 }))
    const header = new Uint8Array(await file.slice(0, 16).arrayBuffer())
    const sqlite = String.fromCharCode(...header) === 'SQLite format 3\0'
    if (sqlite) {
      if (!spec.binary) throw new Error(t('此登录方式只接受文本凭据文件'))
      const data = await readFileAsDataUrl(file, t('文件读取失败'))
      return { name: file.name, base64: data.slice(data.indexOf(',') + 1) }
    }
    if (file.size > 262144) throw new Error(t('文本凭据文件不能超过 256 KiB'))
    const text = new TextDecoder('utf-8', { fatal: true }).decode(await file.arrayBuffer()).replace(/^\uFEFF/, '')
    if (!text.trim() || text.includes('\0')) throw new Error(t('请选择有效的文本凭据文件'))
    return { name: file.name, text }
  }

  async function select(file: File): Promise<void> {
    const token = ++generation.current
    setError(''); setReading(true)
    const current = () => alive.current && token === generation.current && !latest.current.disabled
    try {
      const item = await read(file)
      if (!current()) return
      if (item.base64) {
        latest.current.onChange('')
        latest.current.onUpload({ name: item.name, base64: item.base64 })
      } else {
        latest.current.onUpload({ name: item.name })
        latest.current.onChange(item.text ?? '')
      }
      setFileName(file.name)
    } catch (cause) { if (current()) setError(cause instanceof Error ? cause.message : t('文件读取失败')) }
    finally { if (current()) setReading(false) }
  }

  /**
   * 多选：逐份读好。第一份照常填进表单，其余各份挂在它的 `batch` 上，提交时一起发。
   * 读不了的只报出来，不影响其余各份。
   */
  async function selectMany(files: File[]): Promise<void> {
    const token = ++generation.current
    setError(''); setReading(true)
    const current = () => alive.current && token === generation.current && !latest.current.disabled
    try {
      const items: CredentialUpload[] = []
      const unreadable: string[] = []
      for (const file of files) {
        try {
          if (!current()) return
          items.push(await read(file))
        } catch (cause) {
          unreadable.push(`${file.name}：${cause instanceof Error ? cause.message : t('文件读取失败')}`)
        }
      }
      if (!current()) return
      const [first, ...rest] = items
      if (first) {
        if (first.base64) {
          latest.current.onChange('')
          latest.current.onUpload({ name: first.name, base64: first.base64, batch: rest })
        } else {
          latest.current.onUpload({ name: first.name, batch: rest })
          latest.current.onChange(first.text ?? '')
        }
        setFileName(first.name)
      }
      if (unreadable.length) {
        // 一份都没读出来时，第一条原因原样给出
        setError(first
          ? `${t('{count} 个文件未能读取：', { count: unreadable.length })}\n${unreadable[0]}`
          : unreadable[0])
      }
    } finally { if (current()) setReading(false) }
  }

  function close(): void { setOpen(false); setDraft('') }
  const textSize = new TextEncoder().encode(draft).length
  const validDraft = Boolean(draft.trim()) && !draft.includes('\0') && textSize <= Math.min(spec.maxBytes, 262144)

  return <div className='plugin-login-field'>
    <Input ref={input} type='file' accept={spec.accept} className='hidden' disabled={disabled || reading} multiple
      onChange={event => {
        const files = Array.from(event.currentTarget.files ?? [])
        event.currentTarget.value = ''
        const [first] = files
        if (!first) return
        void (files.length === 1 ? select(first) : selectMany(files))
      }} />
    <div className='flex flex-wrap items-center gap-2'>
      <Button type='button' variant='outline' disabled={disabled || reading} onClick={() => input.current?.click()}>
        {reading ? <Spinner /> : null}{t('选择凭据文件')}
      </Button>
      <Button type='button' variant='link' disabled={disabled || reading} onClick={() => { setDraft(value); setOpen(true); setError('') }}>
        {t('手动粘贴')}
      </Button>
      {value || upload ? <span className='min-w-0 break-all text-xs text-muted-foreground'>
        {upload?.batch?.length
          ? t('{name} 等 {count} 个文件，将逐个添加', { name: upload.name, count: upload.batch.length + 1 })
          : upload?.name || fileName || t('已填写凭据')}</span> : null}
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
