import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, SegmentedControl } from '@ui'
import { CODING_BASE, type CodingDirectories, type CodingProject, type CodingTranslate, type CodingTransport } from './assistant-coding-types'
import { CodingIcon } from './assistant-coding-icons'

export function CodingNewProject({ open, onOpenChange, transport, t, onCreated, initialDirectory }: {
  open: boolean; onOpenChange: (open: boolean) => void; transport: CodingTransport; t: CodingTranslate
  onCreated: (project: CodingProject) => void; initialDirectory?: string
}) {
  const [mode, setMode] = useState('existing')
  const [directory, setDirectory] = useState('')
  const [folderName, setFolderName] = useState('')
  const [listing, setListing] = useState<CodingDirectories | null>(null)
  const [browsing, setBrowsing] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const sequence = useRef(0)
  const saving = useRef(false)
  useEffect(() => {
    if (open) { setMode('existing'); setDirectory(initialDirectory || ''); setFolderName(''); setListing(null); setError(''); setBrowsing(false) }
    return () => { sequence.current += 1 }
  }, [open])
  const browse = async (path: string) => {
    const request = ++sequence.current
    setBrowsing(true); setError('')
    try {
      const data = await transport.json<CodingDirectories>('POST', `${CODING_BASE}/projects/directories`, { path })
      if (request === sequence.current) { setListing(data); setDirectory(data.path) }
    } catch (failure) { if (request === sequence.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { if (request === sequence.current) setBrowsing(false) }
  }
  const create = async () => {
    if (saving.current || !directory.trim() || mode === 'create' && !folderName.trim()) return
    saving.current = true; setBusy(true); setError('')
    try {
      const data = await transport.json<{ project: CodingProject }>('POST', `${CODING_BASE}/projects`, {
        directory: directory.trim(), ...(mode === 'create' ? { folderName: folderName.trim() } : {}),
      })
      onCreated(data.project); onOpenChange(false)
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { saving.current = false; setBusy(false) }
  }
  return <Dialog open={open} onOpenChange={value => { if (!busy) onOpenChange(value) }}>
    <DialogContent className='max-w-xl'>
      <DialogHeader className='flex-col items-start'><DialogTitle>{t('新建项目')}</DialogTitle><DialogDescription>{t('一个文件夹就是一个项目，对话会收在项目下面。')}</DialogDescription></DialogHeader>
      <DialogBody className='gap-4'>
        <SegmentedControl value={mode} onValueChange={setMode} disabled={busy} aria-label={t('项目文件夹')}
          className='w-full [&_[data-slot=segmented-item]]:h-8 [&_[data-slot=segmented-item]]:flex-1 [&_[data-slot=segmented-item]]:justify-center'
          options={[{ value: 'existing', label: t('使用已有文件夹') }, { value: 'create', label: t('创建新文件夹') }]} />
        <Field><FieldLabel htmlFor='coding-project-directory'>{mode === 'create' ? t('保存位置') : t('项目文件夹')}</FieldLabel>
          <div className='flex gap-2'><Input id='coding-project-directory' value={directory} disabled={busy || browsing} placeholder={t('输入电脑上的文件夹路径')}
            onChange={event => { setDirectory(event.currentTarget.value); setListing(null) }} />
            <Button variant='outline' disabled={busy || browsing} onClick={() => void browse(directory)}>{browsing ? t('读取中…') : t('浏览')}</Button></div>
          <FieldDescription>{t('选择的是运行 Agent2API 的电脑上的文件夹，不是手机上的文件夹。')}</FieldDescription>
        </Field>
        {listing ? <div className='space-y-2 rounded-lg border border-border p-2'>
          <div className='flex flex-wrap gap-1'>{listing.locations.map(location => <Button key={location.path} size='sm' variant='ghost' disabled={busy || browsing}
            onClick={() => void browse(location.path)}>{location.name === 'home' ? t('用户目录') : location.name}</Button>)}
            {listing.parent ? <Button size='sm' variant='ghost' disabled={busy || browsing} onClick={() => void browse(listing.parent!)}>{t('上一级目录')}</Button> : null}</div>
          <p className='break-all px-2 text-xs text-muted-foreground'>{listing.path}</p>
          <div className='max-h-48 overflow-y-auto overscroll-contain'>{listing.entries.map(entry => <Button key={entry.path} variant='ghost' size='sm' className='h-8 w-full justify-start gap-2 font-normal max-sm:h-10'
            disabled={busy || browsing} onClick={() => void browse(entry.path)}><CodingIcon name='folder' /><span className='truncate'>{entry.name}</span></Button>)}
            {!listing.entries.length ? <p className='px-2 py-3 text-xs text-muted-foreground'>{t('没有可进入的子目录')}</p> : null}
          </div>
          {listing.limited ? <p className='px-2 text-xs text-muted-foreground'>{t('子文件夹较多，仅显示前 500 个；也可以直接输入路径。')}</p> : null}
        </div> : null}
        {mode === 'create' ? <Field><FieldLabel htmlFor='coding-project-name'>{t('文件夹名称')}</FieldLabel><Input id='coding-project-name' value={folderName} disabled={busy}
          onChange={event => setFolderName(event.currentTarget.value)} placeholder={t('例如：Agent2API')} /></Field> : null}
        <p className='text-xs leading-relaxed text-muted-foreground'>{mode === 'create' ? t('会在所选位置创建一个空文件夹，不会覆盖已有文件。') : t('添加项目不会移动或复制文件；已有的 Codex 对话会归入这个项目。')}</p>
        {error ? <Alert variant='destructive'>{error}</Alert> : null}
      </DialogBody>
      <DialogFooter><Button variant='outline' disabled={busy} onClick={() => onOpenChange(false)}>{t('取消')}</Button>
        <Button disabled={busy || browsing || !directory.trim() || mode === 'create' && !folderName.trim()} onClick={() => void create()}>{busy ? t('正在创建…') : t('创建项目')}</Button></DialogFooter>
    </DialogContent>
  </Dialog>
}
