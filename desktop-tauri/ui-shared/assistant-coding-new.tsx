import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { CODING_BASE, type CodingEngine, type CodingMachine, type CodingTranslate, type CodingTransport } from './assistant-coding-types'

type Directory = { name: string; type: string; isGitRepo?: boolean }
export function CodingNewSession({ open, onOpenChange, engine, machines, transport, t, onCreated }: {
  open: boolean; onOpenChange: (open: boolean) => void; engine: CodingEngine; machines: CodingMachine[]
  transport: CodingTransport; t: CodingTranslate; onCreated: (id: string) => void
}) {
  const [directory, setDirectory] = useState('')
  const [browsePath, setBrowsePath] = useState('')
  const [parent, setParent] = useState<string | null>(null)
  const [entries, setEntries] = useState<Directory[] | null>(null)
  const [model, setModel] = useState('')
  const [mode, setMode] = useState('default')
  const [busy, setBusy] = useState(false)
  const [browsing, setBrowsing] = useState(false)
  const [error, setError] = useState('')
  const browsingSequence = useRef(0)
  const creating = useRef(false)
  const machine = machines[0]
  const opened = useRef(false)
  useEffect(() => {
    if (open && !opened.current) {
      setError(''); setMode('default'); setModel(''); setEntries(null); setBrowsing(false)
      setDirectory(machine?.workspaceRoots[0] || '')
    }
    opened.current = open
    return () => { browsingSequence.current += 1 }
  }, [open])
  const browse = async (path: string) => {
    const sequence = ++browsingSequence.current
    setBrowsing(true); setError('')
    try {
      const data = await transport.json<{ entries: Directory[]; path: string; parent: string | null }>('POST', `${CODING_BASE}/machines/local/directories`, { path })
      if (sequence !== browsingSequence.current) return
      setEntries(data.entries); setBrowsePath(data.path); setDirectory(data.path); setParent(data.parent)
    } catch (failure) { if (sequence === browsingSequence.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { if (sequence === browsingSequence.current) setBrowsing(false) }
  }
  const create = async () => {
    if (creating.current || !machine || !directory.trim()) return
    creating.current = true; setBusy(true); setError('')
    try {
      const data = await transport.json<{ sessionId: string }>('POST', `${CODING_BASE}/sessions`, { machineId: 'local', engine, directory, model, permissionMode: mode })
      onCreated(data.sessionId); onOpenChange(false)
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { creating.current = false; setBusy(false) }
  }
  const labels = [{ value: 'default', label: t('项目内读写') }, { value: 'read-only', label: t('只读') }]
  return <Dialog open={open} onOpenChange={value => { if (!busy) onOpenChange(value) }}><DialogContent className='max-w-xl'>
    <DialogHeader><DialogTitle>{t('新建 Codex 会话')}</DialogTitle></DialogHeader>
    <DialogBody className='gap-4'>
      <Field><FieldLabel>{t('运行电脑')}</FieldLabel><p className='text-sm'>{machine ? `${machine.name} · ${machine.platform}` : t('本机 Codex 尚未就绪，请检查设置。')}</p></Field>
      <Field><FieldLabel htmlFor='coding-directory'>{t('项目目录')}</FieldLabel><div className='flex gap-2'>
        <Input id='coding-directory' value={directory} onChange={e => { setDirectory(e.target.value); setEntries(null) }} disabled={busy} />
        <Button variant='outline' disabled={busy || browsing || !machine || !directory} onClick={() => void browse(directory)}>{t('浏览')}</Button>
      </div><FieldDescription>{t('这是 Agent2API 所在电脑上的目录，只能选择设置中允许的目录及其子目录。')}</FieldDescription></Field>
      {machine?.workspaceRoots.length ? <div className='flex flex-wrap gap-1'>
        {machine.workspaceRoots.map(root => <Button key={root} variant='outline' size='sm' className='max-w-full' disabled={busy || browsing} onClick={() => void browse(root)}><span className='truncate'>{root}</span></Button>)}
      </div> : null}
      {entries ? <div className='max-h-48 overflow-auto rounded-md border border-border p-2'>
        {parent ? <Button variant='ghost' size='sm' disabled={busy || browsing} onClick={() => void browse(parent)}>{t('上一级目录')}</Button> : null}
        {entries.map(entry => <Button key={entry.name} variant='ghost' size='sm' className='flex w-full justify-start' disabled={busy || browsing}
          onClick={() => void browse(`${browsePath.replace(/[\\/]+$/, '')}${browsePath.includes('\\') ? '\\' : '/'}${entry.name}`)}>{entry.isGitRepo ? '▣' : '▸'} {entry.name}</Button>)}
        {!entries.length ? <p className='p-2 text-xs text-muted-foreground'>{t('没有可进入的子目录')}</p> : null}
      </div> : null}
      <Field><FieldLabel htmlFor='coding-model'>{t('模型（可选）')}</FieldLabel><Input id='coding-model' value={model} onChange={e => setModel(e.target.value)} disabled={busy} placeholder={t('留空使用本机 Codex 的默认模型')} /></Field>
      <Field><FieldLabel>{t('权限模式')}</FieldLabel><Select value={mode} onValueChange={value => { if (value) setMode(value) }} disabled={busy}>
        <SelectTrigger><SelectValue>{labels.find(item => item.value === mode)?.label}</SelectValue></SelectTrigger>
        <SelectContent>{labels.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
      </Select><FieldDescription>{t('项目内读写允许修改当前项目；需要额外权限时，由你逐次审批，不会自动批准。')}</FieldDescription></Field>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
    </DialogBody>
    <DialogFooter><Button variant='outline' disabled={busy} onClick={() => onOpenChange(false)}>{t('取消')}</Button>
      <Button disabled={busy || !machine || !directory.trim()} onClick={() => void create()}>{busy ? t('正在创建…') : t('创建会话')}</Button></DialogFooter>
  </DialogContent></Dialog>
}
