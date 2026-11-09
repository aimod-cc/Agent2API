import { useEffect, useState } from 'react'
import { Button, Dialog, DialogContent, DialogHeader, DialogTitle, DialogBody, DialogFooter, Textarea, confirmDialog } from '@ui'
import type { Translate } from './assistant-read-types'

type Props = {
  text: string
  disabled?: boolean
  t: Translate
  onEdit?: (text: string) => void | Promise<void>
  onDelete?: () => void | Promise<void>
  onFork?: () => void | Promise<void>
  copyable?: boolean
  forkLabel?: string
  forkDisabled?: boolean
}

function ActionIcon({ action }: { action: 'edit' | 'delete' | 'fork' | 'copy' | 'check' }) {
  return <svg viewBox='0 0 24 24' width='15' height='15' fill='none' stroke='currentColor' strokeWidth='1.8' strokeLinecap='round' strokeLinejoin='round' aria-hidden='true'>
    {action === 'copy' ? <><rect x='8' y='8' width='12' height='12' rx='2' /><path d='M16 8V4H4v12h4' /></>
      : action === 'check' ? <path d='m5 12 4 4L19 6' />
      : action === 'edit' ? <><path d='m15.6 4.6 3.8 3.8L9 18.8l-4.6.8.8-4.6z' /><path d='m13.8 6.4 3.8 3.8' /></>
      : action === 'delete' ? <><path d='M5 7h14M9.5 7V5.2h5V7M7 7l.9 12.4h8.2L17 7M10.5 10.5v5.5M13.5 10.5v5.5' /></>
        : <><circle cx='6' cy='5' r='2' /><circle cx='6' cy='19' r='2' /><circle cx='18' cy='5' r='2' /><path d='M6 7v10M18 7v1a5 5 0 0 1-5 5H6' /></>}
  </svg>
}

export function AssistantMessageActions({ text, disabled, t, onEdit, onDelete, onFork, copyable, forkLabel, forkDisabled }: Props) {
  const [open, setOpen] = useState(false)
  const [draft, setDraft] = useState('')
  const [saving, setSaving] = useState(false)
  const [copied, setCopied] = useState(false)
  useEffect(() => {
    if (!copied) return
    const timer = setTimeout(() => setCopied(false), 2000)
    return () => clearTimeout(timer)
  }, [copied])
  return <>
    <div className='flex items-center gap-1 pt-1 text-muted-foreground' role='group' aria-label={t('消息操作')}>
      {copyable && navigator.clipboard?.writeText ? <Button size='icon-sm' variant='ghost' title={copied ? t('已复制') : t('复制')} aria-label={copied ? t('已复制') : t('复制')}
        onClick={() => { void navigator.clipboard.writeText(text).then(() => setCopied(true)).catch(() => setCopied(false)) }}><ActionIcon action={copied ? 'check' : 'copy'} /></Button> : null}
      {onEdit ? <Button size='icon-sm' variant='ghost' disabled={disabled} title={t('编辑')} aria-label={t('编辑')} onClick={() => { setDraft(text); setOpen(true) }}><ActionIcon action='edit' /></Button> : null}
      {onDelete ? <Button size='icon-sm' variant='ghost' disabled={disabled} title={t('删除')} aria-label={t('删除')} onClick={async () => {
        if (await confirmDialog(t('删除这条消息？模型消息关联的工具结果也会一并删除。'), { title: t('删除消息') })) await onDelete()
      }}><ActionIcon action='delete' /></Button> : null}
      {onFork ? <Button size='icon-sm' variant='ghost' disabled={disabled || forkDisabled} title={forkLabel || t('分叉')} aria-label={forkLabel || t('分叉')} onClick={() => void onFork()}><ActionIcon action='fork' /></Button> : null}
    </div>
    {onEdit ? <Dialog open={open} onOpenChange={value => { if (!saving) setOpen(value) }}>
      <DialogContent>
        <DialogHeader><DialogTitle>{t('编辑消息')}</DialogTitle></DialogHeader>
        <DialogBody><Textarea rows={10} value={draft} onChange={event => setDraft(event.currentTarget.value)} disabled={saving} /></DialogBody>
        <DialogFooter>
          <Button variant='outline' disabled={saving} onClick={() => setOpen(false)}>{t('取消')}</Button>
          <Button disabled={saving || disabled} onClick={async () => {
            setSaving(true)
            try { await onEdit(draft); setOpen(false) } finally { setSaving(false) }
          }}>{t('保存')}</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog> : null}
  </>
}
