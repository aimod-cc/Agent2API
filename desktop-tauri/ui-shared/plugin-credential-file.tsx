import * as React from 'react'
import { Alert, Badge, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Input, Spinner } from '@ui'
import { t } from '../ui-kit/src/i18n'
import type { CredentialFile, Method } from '../ui-islands/src/islands/add-account-dynamic-fields'
import { readFileAsDataUrl } from './file-read'
import { SecretTextarea } from './plugin-secret-textarea'

const ACCOUNT_FILE_FORMAT = 'agent2api-account'
const NATIVE_TEXT_LIMIT = 262144
const SOFTWARE_EXPORT_LIMIT = 16 * 1024 * 1024
const MAX_SELECTED_FILES = 200

export type CredentialUploadItem = {
  id: string
  name: string
  size: number
  text?: string
  base64?: string
  exportKind?: 'single' | 'multi'
  accountCount?: number
}

export type SoftwareExportInfo = {
  kind: 'single' | 'multi'
  accountCount: number
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === 'object' && !Array.isArray(value))
}

function isSoftwareAccountRecord(value: unknown): boolean {
  if (!isPlainObject(value)) return false
  if (value.format === ACCOUNT_FILE_FORMAT) return true
  const hasId = typeof value.id === 'string' && value.id.trim().length > 0
  if (!hasId) return false
  return (typeof value.provider === 'string' && value.provider.trim().length > 0)
    || isPlainObject(value.pluginSecret)
    || typeof value.accessToken === 'string'
    || typeof value.refreshToken === 'string'
    || typeof value.apiKey === 'string'
}

export function inspectSoftwareExport(text: string): SoftwareExportInfo | undefined {
  const trimmed = text.trim()
  if (!trimmed || (trimmed[0] !== '{' && trimmed[0] !== '[')) return undefined
  let parsed: unknown
  try { parsed = JSON.parse(trimmed) } catch { return undefined }
  if (isPlainObject(parsed)) {
    if (parsed.format === ACCOUNT_FILE_FORMAT) return { kind: 'single', accountCount: 1 }
    let root: Record<string, unknown> = parsed
    if (!Object.hasOwn(parsed, 'accounts') && !Object.hasOwn(parsed, 'provider') && isPlainObject(parsed.data) && Array.isArray(parsed.data.accounts)) {
      root = parsed.data
    }
    if (Array.isArray(root.accounts)) {
      return { kind: 'multi', accountCount: root.accounts.length }
    }
    return undefined
  }
  if (Array.isArray(parsed) && parsed.length > 0 && parsed.every(isSoftwareAccountRecord)) {
    return { kind: 'multi', accountCount: parsed.length }
  }
  return undefined
}

export function credentialInputMethod(method: Method, uploads?: CredentialUploadItem[]): Method {
  const field = method.file?.field
  if (!field || !uploads?.length) return method
  if (uploads.length > 1 || uploads[0]?.base64) {
    return { ...method, prompts: method.prompts.filter(prompt => prompt.key !== field) }
  }
  return method
}

export function credentialFilePayloadForItem(
  method: Method,
  item: CredentialUploadItem | undefined,
  fields: Record<string, string>,
): Record<string, string> {
  if (item?.base64 && method.file?.binary) {
    return { authFileB64: item.base64, authFileName: item.name }
  }
  const field = method.file?.field
  if (!field) return {}
  const hasExtraFields = Object.entries(fields).some(([key, value]) => key !== field && key !== 'name' && value.trim())
  if (hasExtraFields) return {}
  if (item?.text !== undefined) {
    return item.name ? { authFileContent: item.text, authFileName: item.name } : { authFileContent: item.text }
  }
  const pasted = fields[field]
  if (pasted && inspectSoftwareExport(pasted)) {
    return { authFileContent: pasted }
  }
  return {}
}

function formatFileSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1048576) return `${(bytes / 1024).toFixed(bytes >= 102400 ? 0 : 1)} KiB`
  return `${(bytes / 1048576).toFixed(1)} MiB`
}

function fileSignature(file: File): string {
  return `${file.name}:${file.size}:${file.lastModified}`
}

async function readCredentialFile(file: File, spec: CredentialFile, id: string): Promise<CredentialUploadItem> {
  if (!file.size) throw new Error(t('文件为空'))
  const header = new Uint8Array(await file.slice(0, 16).arrayBuffer())
  const sqlite = String.fromCharCode(...header) === 'SQLite format 3\0'
  if (sqlite) {
    if (!spec.binary) throw new Error(t('此登录方式只接受文本凭据文件'))
    if (file.size > spec.maxBytes) {
      throw new Error(t('请选择非空且不超过 {n} MiB 的凭据文件', { n: spec.maxBytes / 1048576 }))
    }
    const data = await readFileAsDataUrl(file, t('文件读取失败'))
    return { id, name: file.name, size: file.size, base64: data.slice(data.indexOf(',') + 1) }
  }
  if (file.size > SOFTWARE_EXPORT_LIMIT) {
    throw new Error(t('凭据文件不能超过 16 MiB'))
  }
  const text = new TextDecoder('utf-8', { fatal: true }).decode(await file.arrayBuffer()).replace(/^\uFEFF/, '')
  if (!text.trim() || text.includes('\0')) throw new Error(t('请选择有效的文本凭据文件'))
  const exportInfo = inspectSoftwareExport(text)
  const nativeLimit = Math.min(spec.maxBytes, NATIVE_TEXT_LIMIT)
  if (file.size > nativeLimit && !exportInfo) {
    throw new Error(t('文本凭据文件不能超过 256 KiB'))
  }
  return {
    id,
    name: file.name,
    size: file.size,
    text,
    ...(exportInfo ? { exportKind: exportInfo.kind, accountCount: exportInfo.accountCount } : {}),
  }
}

export function PluginCredentialFile({ spec, value, uploads = [], disabled, onChange, onUploads, onReading }: {
  spec: CredentialFile
  value: string
  uploads?: CredentialUploadItem[]
  disabled?: boolean
  onChange(value: string): void
  onUploads(value: CredentialUploadItem[]): void
  onReading?(value: boolean): void
}): React.ReactElement {
  const input = React.useRef<HTMLInputElement>(null)
  const [open, setOpen] = React.useState(false)
  const [draft, setDraft] = React.useState('')
  const [error, setError] = React.useState('')
  const [reading, setReading] = React.useState(false)
  const generation = React.useRef(0)
  const alive = React.useRef(true)
  const latest = React.useRef({ disabled, uploads, onChange, onUploads })
  latest.current = { disabled, uploads, onChange, onUploads }
  React.useEffect(() => { onReading?.(reading); return () => onReading?.(false) }, [reading, onReading])
  React.useEffect(() => { alive.current = true; return () => { alive.current = false; generation.current++ } }, [])
  React.useEffect(() => {
    if (disabled) { generation.current++; setOpen(false); setDraft(''); setReading(false) }
  }, [disabled])

  function applyUploads(next: CredentialUploadItem[]): void {
    latest.current.onUploads(next)
    if (next.length === 1 && next[0].text !== undefined) {
      latest.current.onChange(next[0].text)
    } else {
      latest.current.onChange('')
    }
  }

  async function selectFiles(files: File[]): Promise<void> {
    if (!files.length) return
    const token = ++generation.current
    setError('')
    setReading(true)
    const current = () => alive.current && token === generation.current && !latest.current.disabled
    const existing = [...latest.current.uploads]
    const existingIds = new Set(existing.map(item => item.id))
    const added: CredentialUploadItem[] = []
    const failures: string[] = []
    let limitHit = false
    try {
      for (const file of files) {
        if (!current()) return
        const id = fileSignature(file)
        if (existingIds.has(id)) continue
        if (existing.length + added.length >= MAX_SELECTED_FILES) {
          limitHit = true
          break
        }
        try {
          const item = await readCredentialFile(file, spec, id)
          if (!current()) return
          existingIds.add(id)
          added.push(item)
        } catch (cause) {
          if (!current()) return
          const reason = cause instanceof Error ? cause.message : t('文件读取失败')
          failures.push(files.length > 1 ? t('{name}：{reason}', { name: file.name, reason }) : reason)
        }
      }
      if (!current()) return
      if (added.length) applyUploads([...existing, ...added])
      if (limitHit) {
        failures.push(t('单次最多选择 {n} 个文件，超出部分已忽略', { n: MAX_SELECTED_FILES }))
      }
      if (failures.length) {
        const preview = failures.slice(0, 3).join('\n')
        const tail = failures.length > 3 ? `\n${t('其余 {n} 个文件未添加', { n: failures.length - 3 })}` : ''
        setError(`${preview}${tail}`)
      }
    } finally {
      if (current()) setReading(false)
    }
  }

  function removeUpload(id: string): void {
    generation.current++
    setError('')
    applyUploads(uploads.filter(item => item.id !== id))
  }

  function clearUploads(): void {
    generation.current++
    setError('')
    applyUploads([])
  }

  function close(): void { setOpen(false); setDraft('') }
  const textSize = new TextEncoder().encode(draft).length
  const draftExport = inspectSoftwareExport(draft)
  const draftLimit = draftExport ? SOFTWARE_EXPORT_LIMIT : Math.min(spec.maxBytes, NATIVE_TEXT_LIMIT)
  const validDraft = Boolean(draft.trim()) && !draft.includes('\0') && textSize <= draftLimit
  const pastedExport = !uploads.length && value ? inspectSoftwareExport(value) : undefined

  return <div className='plugin-login-field'>
    <Input ref={input} type='file' multiple accept={spec.accept} className='hidden' disabled={disabled || reading}
      onChange={event => {
        const files = Array.from(event.currentTarget.files || [])
        event.currentTarget.value = ''
        if (files.length) void selectFiles(files)
      }} />
    <div className='flex flex-wrap items-center gap-2'>
      <Button type='button' variant='outline' disabled={disabled || reading} onClick={() => input.current?.click()}>
        {reading ? <Spinner /> : null}{uploads.length ? t('添加更多文件') : t('选择凭据文件')}
      </Button>
      <Button type='button' variant='link' disabled={disabled || reading} onClick={() => { setDraft(value); setOpen(true); setError('') }}>
        {t('手动粘贴')}
      </Button>
      {uploads.length ? <Button type='button' variant='link' disabled={disabled || reading} onClick={clearUploads}>
        {t('清空')}
      </Button> : null}
      {!uploads.length && value ? <span className='min-w-0 break-all text-xs text-muted-foreground'>
        {pastedExport?.kind === 'multi'
          ? t('已粘贴多账号文件（含 {count} 个账号）', { count: pastedExport.accountCount })
          : t('已填写凭据')}
      </span> : null}
    </div>
    {uploads.length ? <div className='plugin-credential-file-list'>
      {uploads.length > 1 ? <div className='plugin-credential-file-head'>
        <span>{t('已选 {count} 个文件', { count: uploads.length })}</span>
      </div> : null}
      <div className='plugin-credential-file-rows'>
        {uploads.map(item => <div key={item.id} className='plugin-credential-file-row'>
          <div className='plugin-credential-file-meta'>
            <span className='plugin-credential-file-name' title={item.name}>{item.name}</span>
            {item.exportKind === 'multi' ? <Badge variant='secondary' shape='tag'>
              {t('含 {count} 个账号', { count: item.accountCount ?? 0 })}
            </Badge> : null}
          </div>
          <span className='plugin-credential-file-size'>{formatFileSize(item.size)}</span>
          <Button type='button' variant='ghost' size='icon-sm' disabled={disabled || reading}
            aria-label={t('移除文件 {name}', { name: item.name })} onClick={() => removeUpload(item.id)}>
            <svg viewBox='0 0 16 16' className='size-3' fill='none' stroke='currentColor' strokeWidth='1.7' strokeLinecap='round' aria-hidden='true'>
              <path d='m4 4 8 8M12 4l-8 8' />
            </svg>
          </Button>
        </div>)}
      </div>
    </div> : null}
    {error ? <Alert variant='destructive' className='whitespace-pre-wrap break-words'>{error}</Alert> : null}
    <Dialog open={open && !disabled} onOpenChange={next => { if (!next) close() }}>
      <DialogContent overlayForceRender className='w-[min(620px,calc(100vw-32px))]'>
        <DialogHeader><DialogTitle>{t('粘贴凭据内容')}</DialogTitle></DialogHeader>
        <DialogBody><SecretTextarea label={t('凭据内容')} value={draft} rows={9} maxLength={262144}
          autoComplete='off' autoCapitalize='none' spellCheck={false} onChange={event => setDraft(event.currentTarget.value)} />
          {textSize > draftLimit ? <Alert variant='destructive'>{t('文本凭据文件不能超过 256 KiB')}</Alert> : null}
        </DialogBody>
        <DialogFooter>
          <Button type='button' variant='outline' onClick={close}>{t('取消')}</Button>
          <Button type='button' disabled={!validDraft} onClick={() => {
            generation.current++; onUploads([]); onChange(draft); setError(''); close()
          }}>{t('使用此凭据')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  </div>
}
