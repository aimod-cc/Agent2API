import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { CODING_BASE, type CodingEngine, type CodingMachine, type CodingProject, type CodingTranslate, type CodingTransport } from './assistant-coding-types'

export function CodingNewSession({ open, onOpenChange, engine, machines, projects, initialProjectId, transport, t, onCreated, onNewProject }: {
  open: boolean; onOpenChange: (open: boolean) => void; engine: CodingEngine; machines: CodingMachine[]; projects: CodingProject[]; initialProjectId: string
  transport: CodingTransport; t: CodingTranslate; onCreated: (id: string) => void; onNewProject: () => void
}) {
  const [projectId, setProjectId] = useState('')
  const [mode, setMode] = useState('default')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const creating = useRef(false)
  const machine = machines[0]
  const available = projects.filter(project => project.accessible)
  const selected = projects.find(project => project.id === projectId)
  useEffect(() => {
    if (!open) return
    setError(''); setMode('default')
    setProjectId(available.some(project => project.id === initialProjectId) ? initialProjectId : available[0]?.id || '')
  }, [open])
  const create = async () => {
    if (creating.current || !machine || !selected?.accessible) return
    creating.current = true; setBusy(true); setError('')
    try {
      const data = await transport.json<{ sessionId: string }>('POST', `${CODING_BASE}/sessions`, { machineId: 'local', engine, projectId, permissionMode: mode })
      onCreated(data.sessionId); onOpenChange(false)
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { creating.current = false; setBusy(false) }
  }
  const labels = [{ value: 'default', label: t('项目内读写') }, { value: 'read-only', label: t('只读') }]
  return <Dialog open={open} onOpenChange={value => { if (!busy) onOpenChange(value) }}><DialogContent className='max-w-xl'>
    <DialogHeader><DialogTitle>{t('新建 Codex 会话')}</DialogTitle></DialogHeader>
    <DialogBody className='gap-4'>
      <Field><FieldLabel>{t('项目')}</FieldLabel>
        <Select value={projectId || null} onValueChange={value => { if (value) setProjectId(value) }} disabled={busy || !available.length}>
          <SelectTrigger aria-label={t('项目')}><SelectValue>{selected?.name || t('选择项目')}</SelectValue></SelectTrigger>
          <SelectContent>{available.map(project => <SelectItem key={project.id} value={project.id}><span className='flex min-w-0 flex-col'><span>{project.name}</span><span className='truncate text-xs text-muted-foreground'>{project.directory}</span></span></SelectItem>)}</SelectContent>
        </Select>
        {selected ? <FieldDescription className='break-all'>{selected.directory}</FieldDescription> : <FieldDescription>{t('还没有项目，先新建一个项目')}</FieldDescription>}
        <Button variant='ghost' size='sm' className='self-start' disabled={busy} onClick={onNewProject}>{t('新建项目')}</Button>
      </Field>
      <Field><FieldLabel>{t('权限模式')}</FieldLabel><Select value={mode} onValueChange={value => { if (value) setMode(value) }} disabled={busy}>
        <SelectTrigger aria-label={t('权限模式')}><SelectValue>{labels.find(item => item.value === mode)?.label}</SelectValue></SelectTrigger>
        <SelectContent>{labels.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
      </Select><FieldDescription>{t('项目内读写允许修改当前项目；需要额外权限时，由你逐次审批，不会自动批准。')}</FieldDescription></Field>
      <p className='text-xs text-muted-foreground'>{t('模型、思考等级和速度可在会话输入框旁调整。')}</p>
      {machine ? <p className='text-xs text-muted-foreground'>{t('运行电脑')} · {machine.name}</p> : <Alert variant='warning'>{t('本机 Codex 尚未就绪，请检查设置。')}</Alert>}
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
    </DialogBody>
    <DialogFooter><Button variant='outline' disabled={busy} onClick={() => onOpenChange(false)}>{t('取消')}</Button>
      <Button disabled={busy || !machine || !selected?.accessible} onClick={() => void create()}>{busy ? t('正在创建…') : t('创建会话')}</Button></DialogFooter>
  </DialogContent></Dialog>
}
