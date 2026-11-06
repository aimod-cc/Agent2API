import { useEffect, useRef, useState } from 'react'
import { Alert, Button, Dialog, DialogBody, DialogContent, DialogFooter, DialogHeader, DialogTitle, Field, FieldDescription, FieldLabel, Input, Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@ui'
import { CODING_BASE, engineLabel, type CodingEngine, type CodingMachine, type CodingTranslate, type CodingTransport } from './assistant-coding-types'

type Directory = { name: string; type: string; isGitRepo?: boolean }
export function CodingNewSession({ open, onOpenChange, engine, machines, transport, t, onCreated }: {
  open: boolean; onOpenChange: (open: boolean) => void; engine: CodingEngine; machines: CodingMachine[]
  transport: CodingTransport; t: CodingTranslate; onCreated: (id: string) => void
}) {
  const [machineId, setMachineId] = useState('')
  const [directory, setDirectory] = useState('')
  const [browsePath, setBrowsePath] = useState('')
  const [entries, setEntries] = useState<Directory[] | null>(null)
  const [model, setModel] = useState('')
  const [mode, setMode] = useState('default')
  const [available, setAvailable] = useState<boolean | null>(null)
  const [busy, setBusy] = useState(false)
  const [browsing, setBrowsing] = useState(false)
  const [error, setError] = useState('')
  const browsingSequence = useRef(0)
  const creating = useRef(false)
  const machine = machines.find(m => m.id === machineId)
  const opened = useRef(false)
  useEffect(() => {
    if (open && !opened.current) { setError(''); setMode('default'); setModel('') }
    opened.current = open
    if (open && !machines.some(m => m.id === machineId)) setMachineId(machines[0]?.id || '')
  }, [open, machines, machineId])
  useEffect(() => {
    browsingSequence.current += 1
    setBrowsing(false)
    if (!open || !machineId) return
    const controller = new AbortController()
    setAvailable(null); setEntries(null); setError('')
    setDirectory(machine?.workspaceRoots?.[0] || machine?.homeDirectory || '')
    void transport.json<{ agents: Array<{ agent: string; available: boolean }> }>('GET', `${CODING_BASE}/machines/${encodeURIComponent(machineId)}/availability`, undefined, controller.signal)
      .then(data => { if (!controller.signal.aborted) setAvailable(data.agents.some(a => a.agent === engine && a.available)) })
      .catch(failure => { if (!controller.signal.aborted) setError(failure instanceof Error ? failure.message : String(failure)) })
    return () => controller.abort()
  }, [open, machineId, engine, transport, machine?.homeDirectory, machine?.workspaceRoots?.join('|')])
  const browse = async (path: string) => {
    const sequence = ++browsingSequence.current
    setBrowsing(true); setError('')
    try {
      const data = await transport.json<{ entries: Directory[] }>('POST', `${CODING_BASE}/machines/${encodeURIComponent(machineId)}/directories`, { path })
      if (sequence !== browsingSequence.current) return
      setEntries(data.entries.filter(entry => entry.type === 'directory')); setBrowsePath(path); setDirectory(path)
    } catch (failure) { if (sequence === browsingSequence.current) setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { if (sequence === browsingSequence.current) setBrowsing(false) }
  }
  const create = async () => {
    if (creating.current || !machineId || !directory.trim() || available !== true) return
    creating.current = true; setBusy(true); setError('')
    try {
      const data = await transport.json<{ sessionId: string }>('POST', `${CODING_BASE}/sessions`, { machineId, engine, directory, model, permissionMode: mode })
      onCreated(data.sessionId); onOpenChange(false)
    } catch (failure) { setError(failure instanceof Error ? failure.message : String(failure)) }
    finally { creating.current = false; setBusy(false) }
  }
  const parentPath = (path: string) => {
    const root = /^[A-Za-z]:[\\/]?$/.test(path) || path === '/'
    if (root) return path
    const parent = path.replace(/[\\/]+$/, '').replace(/[\\/][^\\/]+$/, '')
    return /^[A-Za-z]:$/.test(parent) ? `${parent}\\` : parent || '/'
  }
  const labels = engine === 'codex' ? [{ value: 'default', label: t('变更前确认') }, { value: 'read-only', label: t('只读') }]
    : [{ value: 'default', label: t('变更前确认') }, { value: 'plan', label: t('规划模式') }]
  return <Dialog open={open} onOpenChange={value => { if (!busy) onOpenChange(value) }}><DialogContent className='max-w-xl'>
    <DialogHeader><DialogTitle>{t('新建 {engine} 会话', { engine: engineLabel(engine, t) })}</DialogTitle></DialogHeader>
    <DialogBody className='gap-4'>
      <Field><FieldLabel>{t('运行电脑')}</FieldLabel><Select value={machineId} onValueChange={value => { if (value) setMachineId(value) }} disabled={busy}>
        <SelectTrigger><SelectValue>{machine?.name || t('选择在线电脑')}</SelectValue></SelectTrigger>
        <SelectContent>{machines.map(m => <SelectItem key={m.id} value={m.id}>{m.name} · {m.platform}</SelectItem>)}</SelectContent>
      </Select></Field>
      {!machines.length ? <Alert variant='warning'>{t('尚无在线电脑。请先启动 HAPI Runner。')}</Alert> : null}
      {available === false ? <Alert variant='warning'>{t('这台电脑尚未安装或正确配置所选编程工具。')}</Alert> : null}
      <Field><FieldLabel htmlFor='coding-directory'>{t('项目目录')}</FieldLabel><div className='flex gap-2'>
        <Input id='coding-directory' value={directory} onChange={e => { setDirectory(e.target.value); setEntries(null) }} disabled={busy} />
        <Button variant='outline' disabled={busy || browsing || !machineId || !directory} onClick={() => void browse(directory)}>{t('浏览')}</Button>
      </div><FieldDescription>{t('这是运行电脑上的目录，不是手机上的目录。访问范围由 HAPI Runner 的目录限制决定。')}</FieldDescription></Field>
      {entries ? <div className='max-h-48 overflow-auto rounded-md border border-border p-2'>
        <Button variant='ghost' size='sm' disabled={busy || browsing} onClick={() => void browse(parentPath(browsePath))}>{t('上一级目录')}</Button>
        {entries.map(entry => <Button key={entry.name} variant='ghost' size='sm' className='flex w-full justify-start' disabled={busy || browsing}
          onClick={() => void browse(`${browsePath.replace(/[\\/]+$/, '')}${browsePath.includes('\\') ? '\\' : '/'}${entry.name}`)}>{entry.isGitRepo ? '▣' : '▸'} {entry.name}</Button>)}
        {!entries.length ? <p className='p-2 text-xs text-muted-foreground'>{t('没有可进入的子目录')}</p> : null}
      </div> : null}
      <Field><FieldLabel htmlFor='coding-model'>{t('模型（可选）')}</FieldLabel><Input id='coding-model' value={model} onChange={e => setModel(e.target.value)} disabled={busy} placeholder={t('留空使用编程工具的默认模型')} /></Field>
      <Field><FieldLabel>{t('权限模式')}</FieldLabel><Select value={mode} onValueChange={value => { if (value) setMode(value) }} disabled={busy}>
        <SelectTrigger><SelectValue>{labels.find(item => item.value === mode)?.label}</SelectValue></SelectTrigger>
        <SelectContent>{labels.map(item => <SelectItem key={item.value} value={item.value}>{item.label}</SelectItem>)}</SelectContent>
      </Select></Field>
      {error ? <Alert variant='destructive'>{error}</Alert> : null}
    </DialogBody>
    <DialogFooter><Button variant='outline' disabled={busy} onClick={() => onOpenChange(false)}>{t('取消')}</Button>
      <Button disabled={busy || available !== true || !directory.trim()} onClick={() => void create()}>{busy ? t('正在创建…') : t('创建会话')}</Button></DialogFooter>
  </DialogContent></Dialog>
}
